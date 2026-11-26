#!/bin/sh
# A stand-in for hc, for tests: no terminals are touched.
#   ls --json --label K=V   lists the terminal $FAKE_HC_LEAD, or none if empty
#   send <address> <text>   appends the address and the text to $FAKE_HC_LOG
# With FAKE_HC_SEND_ERROR=<kind>, send then fails as hc does (exit 5, the
# error on stderr): having typed the text first, unless the kind is
# not_found, which hc raises before typing.
case "$1" in
ls)
    if [ -n "$FAKE_HC_LEAD" ]; then
        printf '{"hidden_finished":0,"terminals":[{"address":"%s","state":"live","turn":1}]}\n' "$FAKE_HC_LEAD"
    else
        echo '{"hidden_finished":0,"terminals":[]}'
    fi
    ;;
send)
    if [ "$FAKE_HC_SEND_ERROR" != not_found ]; then
        printf '>>> send to %s\n%s\n' "$2" "$3" >> "${FAKE_HC_LOG:?set FAKE_HC_LOG}"
    fi
    if [ -n "$FAKE_HC_SEND_ERROR" ]; then
        printf '{"error":{"kind":"%s","message":"the fake hc fails"}}\n' "$FAKE_HC_SEND_ERROR" >&2
        exit 5
    fi
    echo '{"started":true,"status":"working","turn":2}'
    ;;
*)
    # On stderr, as hc puts its errors.
    echo '{"error":{"kind":"usage","message":"the fake hc knows ls and send"}}' >&2
    exit 2
    ;;
esac
