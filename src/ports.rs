use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, str::FromStr};

const MAX_FORWARDS: usize = 16;
const MAX_CONNECTIONS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    pub host_port: u16,
    pub guest_port: u16,
}

impl FromStr for Forward {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (host, guest) = value.split_once(':').context("use HOST_PORT:GUEST_PORT")?;
        ensure!(
            [host, guest]
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
            "use numeric HOST_PORT:GUEST_PORT"
        );
        let forward = Self {
            host_port: host.parse().context("host port must be 1..65535")?,
            guest_port: guest.parse().context("guest port must be 1..65535")?,
        };
        validate(std::slice::from_ref(&forward))?;
        Ok(forward)
    }
}

pub fn validate(forwards: &[Forward]) -> Result<()> {
    ensure!(
        forwards.len() <= MAX_FORWARDS,
        "at most 16 TCP forwards per session"
    );
    let mut hosts = HashSet::new();
    for forward in forwards {
        ensure!(
            forward.host_port != 0 && forward.guest_port != 0,
            "ports must be 1..65535"
        );
        ensure!(
            hosts.insert(forward.host_port),
            "duplicate host port {}",
            forward.host_port
        );
    }
    Ok(())
}

// OCI uses separate internal ports, avoiding collisions with the runtime API.
pub fn internal_port(index: usize) -> u16 {
    assert!(index < MAX_FORWARDS);
    20000 + index as u16
}

fn backend_port(index: usize) -> u16 {
    internal_port(index) + MAX_FORWARDS as u16
}

pub fn qemu_rule(forward: &Forward, index: usize, oci: bool) -> String {
    let port = if oci {
        backend_port(index)
    } else {
        forward.host_port
    };
    // Target only the VM's DHCP address, never private guestfwd helper endpoints.
    format!(
        ",hostfwd=tcp:127.0.0.1:{port}-10.0.2.15:{}",
        forward.guest_port
    )
}

// Bridge peers may be outside the guest subnet. Restricted slirp DHCP omits
// its default gateway, so normalize OCI connections through local QEMU sockets.
// This lives in the existing runtime, with no extra binary or guest component.
pub struct Relays {
    tasks: tokio::task::JoinSet<Result<()>>,
}

impl Relays {
    pub async fn start(forwards: &[Forward]) -> Result<Self> {
        validate(forwards)?;
        let mut listeners = Vec::new();
        for index in 0..forwards.len() {
            let listener = tokio::net::TcpListener::bind((
                std::net::Ipv4Addr::UNSPECIFIED,
                internal_port(index),
            ))
            .await
            .with_context(|| format!("binding internal TCP forward {}", internal_port(index)))?;
            listeners.push((listener, backend_port(index)));
        }
        Ok(Self::from_listeners(listeners))
    }

    fn from_listeners(listeners: Vec<(tokio::net::TcpListener, u16)>) -> Self {
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
        let mut tasks = tokio::task::JoinSet::new();
        for (listener, backend) in listeners {
            let permits = permits.clone();
            tasks.spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut incoming, _) = accepted.context("accepting TCP forward")?;
                            let Ok(permit) = permits.clone().try_acquire_owned() else {
                                // No unbounded userspace queue; excess connections close.
                                continue;
                            };
                            connections.spawn(async move {
                                let connected = tokio::time::timeout(std::time::Duration::from_secs(5),
                                    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, backend))).await;
                                if let Ok(Ok(mut outgoing)) = connected {
                                    // Default bounded buffers preserve half-close semantics.
                                    let _ = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing).await;
                                }
                                // Keep the slot until join_next retires the receipt,
                                // bounding completed tasks as well as active sockets.
                                permit
                            });
                        }
                        completed = connections.join_next(), if !connections.is_empty() => {
                            if let Some(result) = completed { drop(result.context("TCP forward worker panicked")?); }
                        }
                    }
                }
            });
        }
        Self { tasks }
    }

    pub async fn failure(&mut self) -> Result<()> {
        match self.tasks.join_next().await {
            Some(result) => result.context("TCP forward listener panicked")?,
            None => std::future::pending().await,
        }
    }
}

pub fn publication(forward: &Forward, index: usize) -> String {
    format!(
        "127.0.0.1:{}:{}/tcp",
        forward.host_port,
        internal_port(index)
    )
}

pub fn public(forwards: &[Forward]) -> Value {
    json!(
        forwards
            .iter()
            .map(|forward| json!({
                "protocol":"tcp", "host":format!("127.0.0.1:{}", forward.host_port),
                "guest_port":forward.guest_port
            }))
            .collect::<Vec<_>>()
    )
}

pub fn check_available(forwards: &[Forward]) -> Result<()> {
    validate(forwards)?;
    // Hold all probes together to detect conflicts. QEMU/the engine performs
    // the authoritative bind after these are released; this is not a reservation.
    let _sockets = forwards
        .iter()
        .map(|forward| {
            let probe = || -> std::io::Result<tokio::net::TcpSocket> {
                let socket = tokio::net::TcpSocket::new_v4()?;
                // Match a reusable service listener: a previous connection's
                // TIME_WAIT must not prevent a stopped VM from restarting.
                socket.set_reuseaddr(true)?;
                socket.bind((std::net::Ipv4Addr::LOCALHOST, forward.host_port).into())?;
                Ok(socket)
            };
            probe().with_context(|| {
                format!(
                    "cannot bind host TCP port 127.0.0.1:{}; choose another port",
                    forward.host_port
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn strict_ports_and_bounded_unique_host_bindings() {
        let forward: Forward = "8080:80".parse().unwrap();
        assert_eq!(forward.host_port, 8080);
        for invalid in [
            "0:80",
            "80:0",
            "65536:80",
            "-1:80",
            "+80:80",
            "80",
            "80:80:80",
            "localhost:80:80",
            "80:80,hostfwd=x",
        ] {
            assert!(invalid.parse::<Forward>().is_err(), "{invalid}");
        }
        assert!(validate(&[forward.clone(), forward.clone()]).is_err());
        assert!(
            validate(&[
                forward.clone(),
                Forward {
                    host_port: 8081,
                    guest_port: 80
                }
            ])
            .is_ok()
        );
        assert!(validate(&vec![forward; MAX_FORWARDS + 1]).is_err());
    }

    #[test]
    fn native_and_oci_keep_host_publication_on_loopback() {
        let forward: Forward = "9841:8080".parse().unwrap();
        assert_eq!(
            qemu_rule(&forward, 0, false),
            ",hostfwd=tcp:127.0.0.1:9841-10.0.2.15:8080"
        );
        assert_eq!(
            qemu_rule(&forward, 0, true),
            ",hostfwd=tcp:127.0.0.1:20016-10.0.2.15:8080"
        );
        assert_eq!(publication(&forward, 0), "127.0.0.1:9841:20000/tcp");
        assert_eq!(internal_port(MAX_FORWARDS - 1), 20015);
        assert_eq!(public(&[forward])[0]["host"], "127.0.0.1:9841");
    }

    #[test]
    fn occupied_port_is_reported_with_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let error = check_available(&[Forward {
            host_port: port,
            guest_port: 80,
        }])
        .unwrap_err();
        assert!(error.to_string().contains(&format!("127.0.0.1:{port}")));
    }

    #[tokio::test]
    async fn reusable_listener_is_still_detected_and_stopped_port_can_rebind() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_reuseaddr(true).unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(1).unwrap();
        let addr = listener.local_addr().unwrap();
        let forwards = [Forward {
            host_port: addr.port(),
            guest_port: 80,
        }];
        assert!(check_available(&forwards).is_err());
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        server.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        drop(client);
        server.read_to_end(&mut bytes).await.unwrap();
        drop(server);
        drop(listener);
        check_available(&forwards).unwrap();
    }

    #[tokio::test]
    async fn relay_preserves_binary_streams_half_close_and_listener_shutdown() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_port = backend.local_addr().unwrap().port();
        let front = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = front.local_addr().unwrap();
        let mut relays = Relays::from_listeners(vec![(front, backend_port)]);
        let server = tokio::spawn(async move {
            let (mut stream, _) = backend.accept().await.unwrap();
            assert!(stream.peer_addr().unwrap().ip().is_loopback());
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            stream.write_all(&bytes).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let payload = vec![0xa5; 1_048_576];
        client.write_all(&payload).await.unwrap();
        client.shutdown().await.unwrap();
        let mut result = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_to_end(&mut result),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, payload);
        server.await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), relays.failure())
                .await
                .is_err()
        );
        relays.tasks.abort_all();
        while relays.tasks.join_next().await.is_some() {}
        assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    }

    #[tokio::test]
    async fn relay_connections_are_bounded_and_refusal_closes_clients() {
        use tokio::io::AsyncReadExt;
        let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_port = backend.local_addr().unwrap().port();
        let front = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = front.local_addr().unwrap();
        let relays = Relays::from_listeners(vec![(front, backend_port)]);
        let mut clients = Vec::new();
        let mut peers = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            clients.push(tokio::net::TcpStream::connect(addr).await.unwrap());
            peers.push(
                tokio::time::timeout(std::time::Duration::from_secs(5), backend.accept())
                    .await
                    .unwrap()
                    .unwrap()
                    .0,
            );
        }
        let mut excess = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut byte = [0; 1];
        let count = tokio::time::timeout(std::time::Duration::from_secs(5), excess.read(&mut byte))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, 0);
        drop(peers);
        drop(clients);
        drop(backend);
        drop(relays);
        let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed = unused.local_addr().unwrap().port();
        drop(unused);
        let front = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = front.local_addr().unwrap();
        let _relays = Relays::from_listeners(vec![(front, closed)]);
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
}
