#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in git rg cargo node; do
    command -v "$tool" >/dev/null || { echo "Required check tool missing: $tool" >&2; exit 1; }
done
git rev-parse --is-inside-work-tree >/dev/null

# Include prospective public files, even before the first commit. Tracked files
# are checked regardless of ignore rules, so accidental staging cannot hide them.
while IFS= read -r -d '' public_file; do
    case "$public_file" in
        notes/*|target/*|.artifacts/*|*.iso|*.qcow2|*.partial|windows-client.json|desktop.png)
            echo "Private or generated file in public inventory: $public_file" >&2
            exit 1
            ;;
    esac
    if [ -f "$public_file" ] && rg -n '/home/[[:alnum:]_.-]+/|/Users/[[:alnum:]_.-]+/|\]\((\.\./)*notes/' -- "$public_file"; then
        echo "Machine-specific path or private-note link in $public_file" >&2
        exit 1
    fi
done < <(git ls-files -z --cached --others --exclude-standard)

for probe_file in notes/privacy-probe.md .artifacts/privacy-probe.json windows-client.json desktop.png; do
    git check-ignore -q --no-index "$probe_file"
done

package="$(cargo package --locked --list --allow-dirty)"
if rg -n '^(notes/|target/|\.artifacts/)|\.(iso|qcow2|partial)$|^(windows-client\.json|desktop\.png)$' <<<"$package"; then
    echo 'Private or generated file in crate package' >&2
    exit 1
fi

for script in tests/*.sh; do
    bash -n "$script"
done
node --check assets/viewer.js
node --check tests/viewer-smoke.cjs
echo 'Repository checks passed'
