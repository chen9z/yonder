#!/bin/sh
# A stand-in for hc, for tests: no terminals are touched.
#   ls --json --label K=V   lists the terminal $FAKE_HC_LEAD, or none if empty
#   send <address> <text>   appends the address and the text to $FAKE_HC_LOG
# With FAKE_HC_SEND_ERROR=<kind>, send then fails as hc does (exit 5, the
# error on stderr): having typed the text first, unless the kind is
# not_found, which hc raises before typing.
#   file read [--json] [--on M] <path>   prints {"first","last","lines","text"}
#   file write [--on M] <path>           the file is what comes on stdin
# Files are kept under $FAKE_HC_ROOT/<M>/ (M is local without --on): a
# relative path under its home/, an absolute one as it is beneath it. A
# missing file is not_found, exit 3, as hc says it; with
# FAKE_HC_FILE_ERROR=<kind>, read and write fail with that kind, exit 3.
# Each write appends ">>> file write --on <M> <path>" to $FAKE_HC_LOG.
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
file)
    verb=$2; shift 2
    machine=local
    while [ $# -gt 1 ]; do
        case "$1" in
        --json) shift ;;
        --on) machine=$2; shift 2 ;;
        *) break ;;
        esac
    done
    path=$1
    case "$path" in
    /*) real=${FAKE_HC_ROOT:?set FAKE_HC_ROOT}/$machine$path ;;
    *) real=${FAKE_HC_ROOT:?set FAKE_HC_ROOT}/$machine/home/$path ;;
    esac
    if [ -n "$FAKE_HC_FILE_ERROR" ]; then
        printf '{"error":{"kind":"%s","message":"the fake hc fails"}}\n' "$FAKE_HC_FILE_ERROR" >&2
        exit 3
    fi
    case "$verb" in
    read)
        if [ ! -f "$real" ]; then
            printf '{"error":{"hint":"check the path","kind":"not_found","message":"no file %s"}}\n' "$path" >&2
            exit 3
        fi
        lines=$(wc -l < "$real" | tr -d ' ')
        # A last line without a newline is a line too.
        if [ -s "$real" ] && [ -n "$(tail -c 1 "$real")" ]; then lines=$((lines + 1)); fi
        text=$(perl -0777 -pe 's/\\/\\\\/g; s/"/\\"/g; s/\n/\\n/g; s/\r/\\r/g; s/\t/\\t/g; s/([\x00-\x1f])/sprintf("\\u%04x", ord($1))/ge' "$real")
        printf '{"first":1,"last":%s,"lines":%s,"text":"%s"}\n' "$lines" "$lines" "$text"
        ;;
    write)
        printf '>>> file write --on %s %s\n' "$machine" "$path" >> "${FAKE_HC_LOG:?set FAKE_HC_LOG}"
        mkdir -p "$(dirname "$real")"
        cat > "$real"
        printf '{"bytes":%s,"path":"%s"}\n' "$(wc -c < "$real" | tr -d ' ')" "$path"
        ;;
    *)
        echo '{"error":{"kind":"usage","message":"the fake hc reads and writes files"}}' >&2
        exit 2
        ;;
    esac
    ;;
*)
    # On stderr, as hc puts its errors.
    echo '{"error":{"kind":"usage","message":"the fake hc knows ls, send and file"}}' >&2
    exit 2
    ;;
esac
