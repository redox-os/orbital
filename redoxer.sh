#!/bin/sh

set -e

if [ "$(uname)" != "Redox" ]
then
    redoxer build --verbose
    redoxer build --verbose --examples
    exec redoxer exec --folder . --gui -- sh -- ./redoxer.sh simple
fi

old_path="file:/bin/orbital"
new_path="target/x86_64-unknown-redox/debug/orbital"
if [ -e "${new_path}" ]
then
    mv -v "${new_path}" "${old_path}"
    shutdown --reboot
fi

while [ "$#" != "0" ]
do
    example="$1"
    shift

    echo "# ${example} #"
    "target/x86_64-unknown-redox/debug/examples/${example}"
done
