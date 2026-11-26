#!/bin/sh
# A stand-in for hc, for tests: no terminals are touched.
#   ls --json --label K=V   lists the terminal $FAKE_HC_LEAD, or none if empty
#   send <address> <text>   appends the address and the text to $FAKE_HC_LOG
case "$1" in
ls)
    if [ -n "$FAKE_HC_LEAD" ]; then
        printf '{"hidden_finished":0,"terminals":[{"address":"%s","state":"live","turn":1}]}\n' "$FAKE_HC_LEAD"
    else
        echo '{"hidden_finished":0,"terminals":[]}'
    fi
    ;;
send)
    printf '>>> send to %s\n%s\n' "$2" "$3" >> "${FAKE_HC_LOG:?set FAKE_HC_LOG}"
    echo '{"started":true,"status":"working","turn":2}'
    ;;
*)
    # On stderr, as hc puts its errors.
    echo '{"error":{"kind":"usage","message":"the fake hc knows ls and send"}}' >&2
    exit 2
    ;;
esac
