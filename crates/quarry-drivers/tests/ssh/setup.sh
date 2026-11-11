#!/usr/bin/env bash
# Starts a throw-away sshd on 127.0.0.1:2222 for the SSH tunnel tests.
#
#   tests/ssh/setup.sh          # idempotent: generates keys once, (re)starts sshd
#   tests/ssh/setup.sh stop     # stops it
#
# Needs root (sshd authenticates a real local user for password logins) and the
# openssh-server / openssh-client packages. Everything lives under $DIR.
set -euo pipefail

DIR="${QUARRY_TEST_SSH_DIR:-${TMPDIR:-/tmp}/quarry-ssh-test}"
PORT="${QUARRY_TEST_SSH_PORT:-2222}"
USER_NAME="${QUARRY_TEST_SSH_USER:-quarrytest}"
PASSWORD="quarry-test-pw"

if [ "${1:-}" = "stop" ]; then
  [ -f "$DIR/sshd.pid" ] && kill "$(cat "$DIR/sshd.pid")" 2>/dev/null || true
  rm -f "$DIR/sshd.pid"
  exit 0
fi

mkdir -p "$DIR" /run/sshd
chmod 755 "$DIR"
cd "$DIR"

[ -f host_ed25519 ] || ssh-keygen -q -t ed25519 -N "" -f host_ed25519
[ -f client_ed25519 ] || ssh-keygen -q -t ed25519 -N "" -f client_ed25519
[ -f client_passphrase ] || ssh-keygen -q -t ed25519 -N "s3cret" -f client_passphrase
[ -f client_unauthorized ] || ssh-keygen -q -t ed25519 -N "" -f client_unauthorized
[ -f client_rsa ] || ssh-keygen -q -t rsa -b 3072 -N "" -f client_rsa
cat client_ed25519.pub client_passphrase.pub client_rsa.pub > authorized_keys
chmod 644 authorized_keys

id "$USER_NAME" >/dev/null 2>&1 || useradd -m -s /bin/bash "$USER_NAME"
echo "$USER_NAME:$PASSWORD" | chpasswd

cat > sshd_config <<CFG
ListenAddress 127.0.0.1
Port $PORT
HostKey $DIR/host_ed25519
PidFile $DIR/sshd.pid
AuthorizedKeysFile $DIR/authorized_keys
StrictModes no
UsePAM no
PasswordAuthentication yes
PubkeyAuthentication yes
AllowTcpForwarding yes
AllowUsers $USER_NAME
LogLevel ERROR
CFG

[ -f sshd.pid ] && kill "$(cat sshd.pid)" 2>/dev/null || true
sleep 0.3
/usr/sbin/sshd -f "$DIR/sshd_config" -p "$PORT"
echo "sshd listening on 127.0.0.1:$PORT (user $USER_NAME, password $PASSWORD, keys in $DIR)"
