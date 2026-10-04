using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

namespace Wdesk {
  // A bounded I/O lane independent of the interactive console's STA dispatcher.
  public sealed class Files : IDisposable {
    const long MaxFile = 4294967296;
    const int BufferSize = 262144;
    sealed class Transfer {
      internal string Id, Path, Direction, Hash, Phase, Error, Stage, RangeId;
      internal long Size, Offset;
      internal DateTime Touched = DateTime.UtcNow;
      internal FileStream Stream;
      internal SafeFileHandle Parent;
      internal readonly object IO = new object();
      internal bool Active;
      internal volatile bool Cancelled, Closed;
    }
    readonly object gate = new object();
    readonly Dictionary<string,Transfer> transfers = new Dictionary<string,Transfer>();
    readonly SemaphoreSlim workers = new SemaphoreSlim(2);
    readonly string helper;
    readonly Timer timer;
    readonly Task wire;
    TcpClient connection;
    string active;
    volatile bool disposed;

    public Files(string helperId) {
      helper = helperId;
      timer = new Timer(delegate { Reap(); }, null, 60000, 60000);
      wire = Task.Factory.StartNew(Wire, TaskCreationOptions.LongRunning);
    }
    static void ValidateId(string id) {
      Guid parsed;
      if (id == null || !Guid.TryParseExact(id,"D",out parsed) || parsed.ToString()!=id)
        throw new ArgumentException("Invalid transfer id");
    }
    void CheckHelper(string id) {
      if (id != helper) throw new InvalidOperationException("Helper restarted; transfer incarnation expired");
    }
    Transfer Find(string id) {
      Transfer t;
      if (!transfers.TryGetValue(id,out t)) throw new InvalidOperationException("Unknown or expired transfer");
      return t;
    }
    object Snapshot(Transfer t) {
      return new { id=t.Id, helper_id=helper, path=t.Path, direction=t.Direction,
        size=t.Size, offset=Interlocked.Read(ref t.Offset), sha256=t.Hash,
        phase=t.Phase, active=t.Active, range_id=t.RangeId, error=t.Error, durable=false };
    }
    public object Status(string id,string helperId) {
      CheckHelper(helperId);
      lock(gate) { return Snapshot(Find(id)); }
    }
    public object Begin(string id,string helperId,string direction,string path,long size,string hash) {
      CheckHelper(helperId); ValidateId(id);
      if (direction!="upload" && direction!="download") throw new ArgumentException("Invalid direction");
      if (size<0 || size>MaxFile) throw new ArgumentException("File exceeds 4 GiB");
      if (direction=="upload" && (hash==null || !System.Text.RegularExpressions.Regex.IsMatch(hash,"^[a-f0-9]{64}$")))
        throw new ArgumentException("Expected source SHA-256");
      lock(gate) {
        if (disposed) throw new ObjectDisposedException("Files");
        Transfer previous;
        if (transfers.TryGetValue(id,out previous)) {
          if (previous.Path!=path || previous.Direction!=direction ||
              (direction=="upload" && (previous.Size!=size || previous.Hash!=hash)))
            throw new ArgumentException("Transfer id reused with different source/destination");
          return Snapshot(previous);
        }
        int count=0;
        Transfer oldest=null;
        foreach(Transfer item in transfers.Values) {
          if(!item.Closed)count++;
          else if(oldest==null || item.Touched<oldest.Touched)oldest=item;
        }
        if(count>=8)throw new InvalidOperationException("Eight active transfers; cancel or wait for expiry");
        if(transfers.Count>=256 && oldest!=null)transfers.Remove(oldest.Id);
        Native.ScopedPath(path,false);
        var t=new Transfer {Id=id,Path=path,Direction=direction,Size=size,Hash=hash,Phase="ready"};
        try {
          if(direction=="upload") {
            t.Parent=TransferNative.Parent(path);
            t.Stage="C:\\ProgramData\\wdesk\\staging\\"+id+".bulk";
            t.Stream=TransferNative.Stage(t.Stage);
          } else {
            t.Stream=Native.OpenScoped(path,FileMode.Open,FileAccess.Read);
            t.Size=t.Stream.Length;
            if(t.Size>MaxFile)throw new ArgumentException("File exceeds 4 GiB");
            t.Phase="preparing";
          }
          transfers.Add(id,t);
        } catch {
          if(t.Stream!=null)t.Stream.Dispose();
          if(t.Parent!=null)t.Parent.Dispose();
          if(t.Stream!=null && t.Stage!=null && File.Exists(t.Stage))File.Delete(t.Stage);
          throw;
        }
        if(direction=="download")Background(t,delegate {
          t.Hash=Hash(t);
          lock(gate){if(t.Cancelled)throw new OperationCanceledException();t.Phase="ready";}
        });
        return Snapshot(t);
      }
    }
    string Hash(Transfer t) {
      t.Stream.Position=0;
      using(var hash=SHA256.Create()) {
        byte[] buffer=new byte[BufferSize];int n;
        while((n=t.Stream.Read(buffer,0,buffer.Length))>0) {
          if(t.Cancelled)throw new OperationCanceledException();
          hash.TransformBlock(buffer,0,n,buffer,0);
          lock(gate){t.Touched=DateTime.UtcNow;}
        }
        hash.TransformFinalBlock(new byte[0],0,0);
        return BitConverter.ToString(hash.Hash).Replace("-","").ToLowerInvariant();
      }
    }
    void Background(Transfer t,Action action) {
      Task.Run(async delegate {
        await workers.WaitAsync();
        try {
          lock(t.IO) {
            try {if(t.Cancelled)throw new OperationCanceledException();action();}
            catch(Exception e) {
              lock(gate){if(t.Phase!="committed")t.Phase=t.Cancelled?"cancelled":"failed";t.Error=e.Message;t.Touched=DateTime.UtcNow;}
              Close(t);
            }
          }
        } finally {workers.Release();}
      });
    }
    public object Commit(string id,string helperId) {
      CheckHelper(helperId);
      lock(gate) {
        var t=Find(id);
        if(t.Phase=="committed" || t.Phase=="verifying")return Snapshot(t);
        if(t.Cancelled || t.Active || t.Closed || t.Phase!="ready" || t.Offset!=t.Size)
          throw new InvalidOperationException("Transfer is not complete/idle");
        t.Phase="verifying";t.Touched=DateTime.UtcNow;
        Background(t,delegate {
          if(t.Direction=="upload") {
            if(Hash(t)!=t.Hash)throw new InvalidDataException("Transfer SHA-256 mismatch; destination unchanged");
            t.Stream.Flush(true);
          }
          lock(gate) {
            if(t.Cancelled)throw new OperationCanceledException();
            if(t.Direction=="upload")TransferNative.Promote(t.Stream,t.Parent,t.Path);
            t.Phase="committed";t.Touched=DateTime.UtcNow;
          }
          Close(t);
        });
        return Snapshot(t);
      }
    }
    public object Cancel(string id,string helperId) {
      CheckHelper(helperId);
      lock(gate) {
        var t=Find(id);
        if(t.Closed || t.Phase=="committed" || t.Phase=="cancelling")return Snapshot(t);
        t.Cancelled=true;t.Phase="cancelling";t.Touched=DateTime.UtcNow;
        if(active==id && connection!=null)connection.Close();
        Background(t,delegate {Close(t);lock(gate){t.Phase="cancelled";}});
        return Snapshot(t);
      }
    }
    public object Pause(string id,string helperId) {
      CheckHelper(helperId);
      lock(gate) {
        var t=Find(id);
        if(active==id && connection!=null)connection.Close();
        return Snapshot(t);
      }
    }
    void Close(Transfer t) {
      if(t.Closed)return;
      try {
        if(t.Stream!=null)t.Stream.Dispose();
      } finally {
        if(t.Parent!=null)t.Parent.Dispose();
        try {if(t.Stage!=null && File.Exists(t.Stage))File.Delete(t.Stage);}
        catch(IOException e){lock(gate){t.Error=e.Message;}}
        finally {lock(gate){t.Closed=true;}}
      }
    }
    void Reap() {
      try {
        var expire=new List<Transfer>();
        lock(gate) {
          foreach(var t in transfers.Values)
            if((DateTime.UtcNow-t.Touched).TotalMinutes>10)expire.Add(t);
          foreach(var t in expire)if(t.Closed)transfers.Remove(t.Id);
        }
        foreach(var t in expire)if(!t.Closed)Cancel(t.Id,helper);
        Sweep();
      } catch { /* Retry on the next tick, without terminating console control. */ }
    }
    void Sweep() {
      string root="C:\\ProgramData\\wdesk\\staging";
      if((File.GetAttributes(root)&FileAttributes.ReparsePoint)!=0)return;
      int inspected=0;
      foreach(string path in Directory.EnumerateFiles(root,"*.bulk",SearchOption.TopDirectoryOnly)) {
        if(++inspected>256)break;
        string id=Path.GetFileNameWithoutExtension(path);Guid parsed;
        if(!Guid.TryParseExact(id,"D",out parsed) || parsed.ToString()!=id)continue;
        lock(gate){Transfer t;if(transfers.TryGetValue(id,out t) && !t.Closed)continue;}
        try {if((File.GetAttributes(path)&FileAttributes.ReparsePoint)==0)File.Delete(path);}
        catch(IOException){} // An active/open stage cannot be deleted.
        catch(UnauthorizedAccessException){}
      }
    }
    static byte[] Exact(Stream stream,int length) {
      var bytes=new byte[length];int offset=0;
      while(offset<length){int n=stream.Read(bytes,offset,length-offset);if(n==0)throw new EndOfStreamException();offset+=n;}
      return bytes;
    }
    static string Text(Stream stream) {return Encoding.ASCII.GetString(Exact(stream,36));}
    static long Number(Stream stream) {return BitConverter.ToInt64(Exact(stream,8),0);}
    static void Reply(Stream stream,string nonce,long offset,string error) {
      byte[] message=Encoding.UTF8.GetBytes(error==null?"":error.Substring(0,Math.Min(error.Length,1024)));
      stream.Write(Encoding.ASCII.GetBytes("WDB1"),0,4);
      stream.Write(Encoding.ASCII.GetBytes(nonce),0,36);
      stream.WriteByte(error==null?(byte)0:(byte)1);
      stream.Write(BitConverter.GetBytes(offset),0,8);
      stream.Write(BitConverter.GetBytes(message.Length),0,4);
      stream.Write(message,0,message.Length);
    }
    void Range(NetworkStream stream) {
      stream.ReadTimeout=Timeout.Infinite; // An idle lane is not a stalled transfer.
      string magic=Encoding.ASCII.GetString(Exact(stream,4));
      stream.ReadTimeout=30000;
      if(magic=="WDP1") {
        string pingHelper=Text(stream),pingNonce=Text(stream);
        ValidateId(pingNonce);
        try {CheckHelper(pingHelper);Reply(stream,pingNonce,0,null);}
        catch(Exception e){Reply(stream,pingNonce,0,e.Message);throw;}
        return;
      }
      if(magic!="WDB1")throw new InvalidDataException("Invalid bulk framing");
      string incarnation=Text(stream),id=Text(stream),nonce=Text(stream);
      ValidateId(id);ValidateId(nonce);
      int direction=stream.ReadByte();long offset=Number(stream),length=Number(stream);
      Transfer t=null;bool accepted=false,claimed=false;
      try {
        CheckHelper(incarnation);
        lock(gate) {
          t=Find(id);
          if(t.Cancelled || t.Active || t.Closed || t.Phase!="ready" || offset<0 || length<0 ||
              offset>t.Size || length>t.Size-offset ||
              (direction==1 && (t.Direction!="upload" || offset!=t.Offset)) ||
              (direction==2 && t.Direction!="download") || (direction!=1 && direction!=2))
            throw new InvalidOperationException("Transfer range is not ready or has an invalid offset");
          t.Active=true;claimed=true;active=id;t.RangeId=nonce;t.Error=null;t.Touched=DateTime.UtcNow;
        }
        lock(t.IO) {
          t.Stream.Position=offset;
          Reply(stream,nonce,offset,null);accepted=true;
          byte[] buffer=new byte[BufferSize];long remaining=length;
          while(remaining>0) {
            if(t.Cancelled)throw new OperationCanceledException();
            int n=(int)Math.Min(buffer.Length,remaining);
            if(direction==1) {
              n=stream.Read(buffer,0,n);if(n==0)throw new EndOfStreamException();
              t.Stream.Write(buffer,0,n);
            } else {
              n=t.Stream.Read(buffer,0,n);if(n==0)throw new EndOfStreamException();
              stream.Write(buffer,0,n);
            }
            remaining-=n;offset+=n;Interlocked.Exchange(ref t.Offset,offset);
            lock(gate){t.Touched=DateTime.UtcNow;}
          }
          if(direction==1)t.Stream.Flush();
          Reply(stream,nonce,offset,null);
        }
      } catch(Exception e) {
        if(t!=null)lock(gate){t.Error=e.Message;}
        if(!accepted)Reply(stream,nonce,offset,e.Message);
        throw; // Never interpret an interrupted range's tail as a command.
      } finally {
        if(claimed)lock(gate){t.Active=false;active=null;t.Touched=DateTime.UtcNow;}
      }
    }
    void Wire() {
      try {Sweep();}catch(IOException){}catch(UnauthorizedAccessException){}
      while(!disposed) {
        var client=new TcpClient();
        try {
          lock(gate){if(disposed){client.Close();break;}connection=client;}
          client.Connect("10.0.2.101",9844);client.NoDelay=true;
          var stream=client.GetStream();stream.ReadTimeout=30000;stream.WriteTimeout=30000;
          while(!disposed)Range(stream);
        } catch(Exception) {if(!disposed)Thread.Sleep(250);}
        finally {client.Close();lock(gate){if(connection==client)connection=null;}}
      }
    }
    public void Dispose() {
      var pending=new List<Transfer>();
      lock(gate){disposed=true;if(connection!=null)connection.Close();foreach(var t in transfers.Values)if(!t.Closed){t.Cancelled=true;pending.Add(t);}}
      timer.Dispose();wire.Wait(1000);
      foreach(var t in pending)Background(t,delegate {Close(t);});
    }
  }
}
