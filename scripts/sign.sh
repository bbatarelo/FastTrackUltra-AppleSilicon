#!/bin/sh
# Local builds use an ad-hoc signature. Release signing is explicit.
set -eu
bundle=${1:?usage: sign.sh bundle}
identity=${SIGN_IDENTITY:--}
if [ "$identity" = - ]; then
    /usr/bin/codesign --force --sign - "$bundle"
else
    /usr/bin/codesign --force --sign "$identity" --options runtime --timestamp "$bundle"
fi
/usr/bin/codesign --verify --strict "$bundle"
