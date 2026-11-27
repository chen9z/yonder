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
# Bots' terminals, kept in $FAKE_HC_ROOT/terms/ (one file each, its labels):
#   run --bg [--json] [--name N] [--cwd D] [--on M] [--label K=V]...
#       [--outlive-parent] [--keep] -- <argv...>
#     logs ">>> run <the options as given> -- <argv>"; addresses go t70,
#     t71, ... (M/tNN on M; the count is in $FAKE_HC_ROOT/next). With
#     FAKE_HC_RUN_EXEC=1 and no --on, argv runs in D, in the background, in
#     a session of its own, its output dropped.
#   get [--json] <t>    live, or finished once killed; status.state is
#                       $FAKE_HC_STATUS (default idle), screen $FAKE_HC_SCREEN
#   read [--json] <t>   its text: $FAKE_HC_SCREEN
#   ls --json --label yonder.bot=X   the terminals carrying that label
#   interrupt <t>, kill <t>          log ">>> interrupt <t>" / ">>> kill <t>";
#                                    kill finishes it
#   exec [--on M] -- <argv...>       logs ">>> exec --on <M> -- <argv>"; runs
#     argv here without --on, else exits $FAKE_HC_EXEC_CODE (default 0)
# Unknown terminals are not_found, exit 3, as hc says it.

log() { printf '%s\n' "$*" >> "${FAKE_HC_LOG:?set FAKE_HC_LOG}"; }
json_text() { printf %s "$1" | perl -0777 -pe 's/\\/\\\\/g; s/"/\\"/g; s/\n/\\n/g; s/\r/\\r/g; s/\t/\\t/g; s/([\x00-\x1f])/sprintf("\\u%04x", ord($1))/ge'; }
term() { printf '%s/terms/%s' "${FAKE_HC_ROOT:?set FAKE_HC_ROOT}" "$(printf %s "$1" | tr / _)"; }
unknown() {
    printf '{"error":{"hint":"see `hc ls --all`","kind":"not_found","message":"no terminal \\"%s\\""}}\n' "$1" >&2
    exit 3
}
term_state() { if [ -e "$(term "$1").finished" ]; then echo finished; else echo live; fi; }

case "$1" in
ls)
    label=
    while [ $# -gt 0 ]; do [ "$1" = --label ] && label=$2; shift; done
    case "$label" in
    yonder.bot=*)
        list=
        for f in "${FAKE_HC_ROOT:?set FAKE_HC_ROOT}"/terms/*; do
            [ -f "$f" ] || continue
            case "$f" in *.finished) continue ;; esac
            [ -e "$f.finished" ] && continue
            grep -qxF "$label" "$f" || continue
            a=$(head -1 "$f")
            list="$list${list:+,}{\"address\":\"$a\",\"state\":\"live\",\"turn\":1}"
        done
        printf '{"hidden_finished":0,"terminals":[%s]}\n' "$list"
        exit 0
        ;;
    esac
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
run)
    shift
    opts= name= cwd=. machine= labels=
    while [ $# -gt 0 ] && [ "$1" != -- ]; do
        opts="$opts $1"
        case "$1" in
        --name|--cwd|--on|--label)
            opts="$opts $2"
            case "$1" in
            --name) name=$2 ;;
            --cwd) cwd=$2 ;;
            --on) machine=$2 ;;
            --label) labels="$labels$2
" ;;
            esac
            shift 2
            ;;
        *) shift ;;
        esac
    done
    [ "$1" = -- ] && shift
    log ">>> run$opts -- $*"
    mkdir -p "$FAKE_HC_ROOT/terms"
    n=$(cat "$FAKE_HC_ROOT/next" 2>/dev/null || echo 70)
    echo $((n + 1)) > "$FAKE_HC_ROOT/next"
    address=t$n
    [ -n "$machine" ] && address=$machine/t$n
    printf '%s\n%s' "$address" "$labels" > "$(term "$address")"
    if [ "$FAKE_HC_RUN_EXEC" = 1 ] && [ -z "$machine" ]; then
        # Comes back once the child has a session of its own (so it outlives
        # this process's group) and has let go of this output.
        (cd "$cwd" && exec perl -MPOSIX -e '
            pipe(my $r, my $w) or die;
            if (my $pid = fork) { close $w; <$r>; exit 0 }
            close $r; POSIX::setsid();
            open STDIN, "<", "/dev/null"; open STDOUT, ">", "/dev/null"; open STDERR, ">&", \*STDOUT;
            close $w; exec @ARGV or exit 127' "$@")
    fi
    printf '{"address":"%s","name":"%s","state":"live","turn":0}\n' "$address" "$name"
    ;;
get|read)
    verb=$1; shift
    [ "$1" = --json ] && shift
    [ -f "$(term "$1")" ] || unknown "$1"
    screen=$(json_text "$FAKE_HC_SCREEN")
    if [ "$verb" = read ]; then
        printf '{"complete":true,"how":"screen","text":"%s"}\n' "$screen"
    else
        printf '{"address":"%s","state":"%s","status":{"app":"fake","state":"%s"},"screen":"%s"}\n' \
            "$1" "$(term_state "$1")" "${FAKE_HC_STATUS:-idle}" "$screen"
    fi
    ;;
interrupt|kill)
    verb=$1; shift
    [ "$1" = --json ] && shift
    log ">>> $verb $1"
    [ -f "$(term "$1")" ] || unknown "$1"
    if [ "$verb" = kill ]; then
        : > "$(term "$1").finished"
        printf '{"address":"%s","state":"finished"}\n' "$1"
    elif [ "$(term_state "$1")" = finished ]; then
        printf '{"error":{"hint":"hc read %s","kind":"finished","message":"%s has finished"}}\n' "$1" "$1" >&2
        exit 11
    else
        echo '{"key":"ctrl+c","outcome":"unknown"}'
    fi
    ;;
exec)
    shift
    machine=
    while [ $# -gt 0 ] && [ "$1" != -- ]; do
        case "$1" in
        --on) machine=$2; shift 2 ;;
        *) shift ;;
        esac
    done
    [ "$1" = -- ] && shift
    log ">>> exec --on ${machine:-local} -- $*"
    if [ -z "$machine" ]; then
        "$@"
        exit $?
    fi
    exit "${FAKE_HC_EXEC_CODE:-0}"
    ;;
*)
    # On stderr, as hc puts its errors.
    echo '{"error":{"kind":"usage","message":"the fake hc knows ls, send, file, run, get, read, interrupt, kill and exec"}}' >&2
    exit 2
    ;;
esac
