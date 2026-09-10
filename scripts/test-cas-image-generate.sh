#!/usr/bin/env bash
# Security regression test for the cas-image-generate curl credential path.
#
# The stub curl wrapper delegates to a local HTTP endpoint so the test observes
# the real curl process command line while the endpoint deliberately holds the
# request open. The API key must be present in the mode-600 header file, never
# in curl argv, and the file must be removed when the helper exits.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmpdir="$(mktemp -d)"
server_pid=""
script_pid=""

cleanup() {
    if [[ -n "${script_pid}" ]]; then
        kill "${script_pid}" 2>/dev/null || true
        wait "${script_pid}" 2>/dev/null || true
    fi
    if [[ -n "${server_pid}" ]]; then
        kill "${server_pid}" 2>/dev/null || true
        wait "${server_pid}" 2>/dev/null || true
    fi
    rm -rf "${tmpdir}"
}
trap cleanup EXIT

stub_bin="${tmpdir}/bin"
mkdir -p "${stub_bin}"

cat >"${tmpdir}/server.py" <<'PY'
import http.server
import os
import pathlib
import sys
import time

port_path = pathlib.Path(sys.argv[1])
response_marker = pathlib.Path(sys.argv[2])
expected_key = os.environ["EXPECTED_KEY"]


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        if self.headers.get("x-goog-api-key") != expected_key or not body:
            self.send_error(400, "header or payload mismatch")
            return

        response_marker.touch()
        # Keep curl alive long enough for the shell test to inspect its argv.
        time.sleep(2)
        payload = (
            '{"candidates":[{"content":{"parts":[{"inlineData":'
            '{"mimeType":"image/png","data":"c3R1Yi1pbWFnZQ=="}}]}}]}'
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
port_path.write_text(str(server.server_port))
server.serve_forever()
PY

cat >"${stub_bin}/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

args=("$@")
header_file=""
for ((index = 0; index < ${#args[@]}; index++)); do
    if [[ "${args[index]}" == "-H" && $((index + 1)) -lt ${#args[@]} ]]; then
        header_spec="${args[index + 1]}"
        if [[ "${header_spec}" == @* ]]; then
            header_file="${header_spec#@}"
        fi
    fi
done
[[ -n "${header_file}" ]] || { echo "stub curl did not receive a header file" >&2; exit 1; }
printf '%s\n' "${BASHPID}" >"${CURL_STUB_PID_FILE}"
printf '%s\n' "${header_file}" >"${CURL_STUB_HEADER_FILE}"
last_index=$((${#args[@]} - 1))
args[last_index]="${CURL_STUB_URL}"
exec "${REAL_CURL}" "${args[@]}"
SH
chmod 755 "${stub_bin}/curl"

run_case() {
    local label="$1" script="$2"
    local port_file="${tmpdir}/${label}.port"
    local response_marker="${tmpdir}/${label}.response"
    local header_capture="${tmpdir}/${label}.header"
    local pid_capture="${tmpdir}/${label}.pid"
    local output="${tmpdir}/${label}.png"
    local secret='gh-757-test-secret-should-not-be-in-argv'
    local port header_file curl_pid curl_args mode

    EXPECTED_KEY="${secret}" python3 -u "${tmpdir}/server.py" \
        "${port_file}" "${response_marker}" &
    server_pid=$!
    for _ in $(seq 1 100); do
        [[ -s "${port_file}" ]] && break
        kill -0 "${server_pid}" 2>/dev/null || {
            wait "${server_pid}" || true
            echo "${label}: stub endpoint exited before it published a port" >&2
            return 1
        }
        sleep 0.02
    done
    [[ -s "${port_file}" ]] || { echo "${label}: stub endpoint did not start" >&2; return 1; }
    port="$(<"${port_file}")"

    GEMINI_API_KEY="${secret}" \
        CURL_STUB_URL="http://127.0.0.1:${port}/generate" \
        CURL_STUB_HEADER_FILE="${header_capture}" \
        CURL_STUB_PID_FILE="${pid_capture}" \
        REAL_CURL="$(command -v curl)" \
        PATH="${stub_bin}:${PATH}" \
        bash "${script}" --prompt "security regression" --output "${output}" &
    script_pid=$!

    for _ in $(seq 1 200); do
        [[ -s "${header_capture}" && -s "${pid_capture}" ]] && break
        kill -0 "${script_pid}" 2>/dev/null || {
            wait "${script_pid}" || true
            echo "${label}: helper exited before curl exposed its header file" >&2
            return 1
        }
        sleep 0.02
    done
    [[ -s "${header_capture}" && -s "${pid_capture}" ]] || {
        echo "${label}: curl inspection files were not published" >&2
        return 1
    }

    header_file="$(<"${header_capture}")"
    curl_pid="$(<"${pid_capture}")"
    [[ -f "${header_file}" ]] || { echo "${label}: header file disappeared too early" >&2; return 1; }
    grep -Fqx "x-goog-api-key: ${secret}" "${header_file}"
    grep -Fqx 'Content-Type: application/json' "${header_file}"
    mode="$(python3 - "${header_file}" <<'PY'
import os
import stat
import sys
print(format(stat.S_IMODE(os.stat(sys.argv[1]).st_mode), "03o"))
PY
)"
    [[ "${mode}" == 600 ]] || { echo "${label}: header mode is ${mode}, expected 600" >&2; return 1; }

    curl_args="$(ps -ww -o args= -p "${curl_pid}")"
    [[ -n "${curl_args}" ]] || { echo "${label}: curl process was not inspectable" >&2; return 1; }
    if [[ "${curl_args}" == *"${secret}"* ]]; then
        echo "${label}: API key leaked into curl argv: ${curl_args}" >&2
        return 1
    fi
    [[ "${curl_args}" == *"@${header_file}"* ]] || {
        echo "${label}: curl argv did not reference the header file: ${curl_args}" >&2
        return 1
    }

    wait "${script_pid}"
    script_pid=""
    [[ -f "${response_marker}" ]] || { echo "${label}: stub endpoint did not receive the request" >&2; return 1; }
    [[ -f "${output}" ]] || { echo "${label}: helper did not write the response image" >&2; return 1; }
    [[ "$(<"${output}")" == stub-image ]] || { echo "${label}: unexpected image response" >&2; return 1; }
    [[ ! -e "${header_file}" ]] || { echo "${label}: header file survived helper exit" >&2; return 1; }

    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
    server_pid=""
    echo "ok   ${label}: key absent from curl argv; header mode 600; cleanup verified"
}

run_case claude "${repo_root}/cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh"
run_case codex "${repo_root}/cas-cli/src/builtins/codex/skills/cas-image-generate/scripts/generate-image.sh"
run_case grok "${repo_root}/cas-cli/src/builtins/grok/skills/cas-image-generate/scripts/generate-image.sh"
echo "PASS: cas-image-generate credential argv and header-file contract verified"
