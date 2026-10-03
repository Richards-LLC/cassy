#!/usr/bin/env bash
# Shared by the runner hooks; pure comparison is also exercised on Darwin.
process_identity_matches() {
    local expected_pid="$1" expected_start="$2" observed_pid="$3" observed_start="$4" state="$5"
    [[ "$expected_pid" =~ ^[1-9][0-9]*$ && "$expected_start" =~ ^[0-9]+$ &&
       "$expected_pid" == "$observed_pid" && "$expected_start" == "$observed_start" &&
       "$state" != Z && "$state" != X && -n "$state" ]]
}

# Fields after the final ')' start at stat field 3. A comm may contain spaces
# or parentheses, so splitting the whole line would misread the start time.
process_snapshot() {
    local pid="$1" stat_line
    local -a fields=()
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
    IFS= read -r stat_line 2>/dev/null <"/proc/$pid/stat" || return 1
    read -r -a fields <<<"${stat_line##*) }"
    [[ ${#fields[@]} -ge 20 && "${fields[19]}" =~ ^[0-9]+$ ]] || return 1
    printf '%s %s %s %s\n' "$pid" "${fields[19]}" "${fields[0]}" "${fields[1]}"
}

process_matches() {
    local pid="$1" start="$2" observed_pid observed_start state parent snapshot
    snapshot="$(process_snapshot "$pid")" || return 1
    read -r observed_pid observed_start state parent <<<"$snapshot"
    process_identity_matches "$pid" "$start" "$observed_pid" "$observed_start" "$state"
}

worker_identity() {
    local pid="$PPID" start state parent snapshot comm
    while [[ "$pid" =~ ^[1-9][0-9]*$ && "$pid" != 1 ]]; do
        snapshot="$(process_snapshot "$pid")" || return 1
        read -r pid start state parent <<<"$snapshot"
        IFS= read -r comm <"/proc/$pid/comm" || return 1
        if [[ "$comm" == Runner.Worker ]]; then
            [[ "$(stat -c %u -- "/proc/$pid")" == "$(id -u)" ]] || return 1
            process_identity_matches "$pid" "$start" "$pid" "$start" "$state" || return 1
            printf '%s %s\n' "$pid" "$start"
            return 0
        fi
        [[ "$parent" != "$pid" ]] || return 1
        pid="$parent"
    done
    return 1
}
