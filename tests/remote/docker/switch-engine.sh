#!/bin/sh
set -eu
# Both sockets belong to the disposable Docker-in-Docker fixture.
restore() {
    if [ -S /engine-run/docker-original.sock ]; then
        rm -f /engine-run/docker.sock
        mv /engine-run/docker-original.sock /engine-run/docker.sock
    fi
    touch /control/restored
}
trap restore EXIT
trap 'exit 1' INT TERM
test -S /engine-run/docker-replacement.sock
test ! -e /engine-run/docker-original.sock
mv /engine-run/docker.sock /engine-run/docker-original.sock
ln -s docker-replacement.sock /engine-run/docker.sock
touch /control/switched
i=0
while [ ! -f /control/release ] && [ "$i" -lt 300 ]; do
    sleep 0.1
    i=$((i+1))
done
