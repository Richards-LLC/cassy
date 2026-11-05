#!/usr/bin/env bash
# Security regression test for the cas-image-generate curl credential path.
#
# The stub curl wrapper records the argv and header file it passes to real curl,
# then delegates to a local HTTP endpoint. Inspect the captures after the helper
# exits so neither process lifetime nor header cleanup can race the assertions.
# The API key must be present in the mode-600 header file, never in curl argv,
# and the file must be removed when the helper exits.

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
printf '%s\n' "${header_file}" >"${CURL_STUB_HEADER_FILE}"
cp "${header_file}" "${CURL_STUB_HEADER_CONTENTS_FILE}"
python3 - "${header_file}" >"${CURL_STUB_HEADER_MODE_FILE}" <<'PY'
import os
import stat
import sys
print(format(stat.S_IMODE(os.stat(sys.argv[1]).st_mode), "03o"))
PY
last_index=$((${#args[@]} - 1))
args[last_index]="${CURL_STUB_URL}"
printf '%s\n' "${args[@]}" >"${CURL_STUB_ARGV_FILE}"
exec "${REAL_CURL}" "${args[@]}"
SH
chmod 755 "${stub_bin}/curl"

run_case() {
    local label="$1" script="$2"
    local port_file="${tmpdir}/${label}.port"
    local response_marker="${tmpdir}/${label}.response"
    local header_capture="${tmpdir}/${label}.header"
    local header_contents_capture="${tmpdir}/${label}.headers"
    local header_mode_capture="${tmpdir}/${label}.mode"
    local argv_capture="${tmpdir}/${label}.argv"
    local output="${tmpdir}/${label}.png"
    local secret='gh-757-test-secret-should-not-be-in-argv'
    local port header_file curl_args mode

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
        CURL_STUB_HEADER_CONTENTS_FILE="${header_contents_capture}" \
        CURL_STUB_HEADER_MODE_FILE="${header_mode_capture}" \
        CURL_STUB_ARGV_FILE="${argv_capture}" \
        REAL_CURL="$(command -v curl)" \
        PATH="${stub_bin}:${PATH}" \
        bash "${script}" --prompt "security regression" --output "${output}" &
    script_pid=$!

    # Regression: even if curl and the helper finish before inspection, the
    # credential assertions must still be inspectable without a timing window.
    wait "${script_pid}"
    script_pid=""

    header_file="$(<"${header_capture}")"
    grep -Fqx "x-goog-api-key: ${secret}" "${header_contents_capture}"
    grep -Fqx 'Content-Type: application/json' "${header_contents_capture}"
    mode="$(<"${header_mode_capture}")"
    [[ "${mode}" == 600 ]] || { echo "${label}: header mode is ${mode}, expected 600" >&2; return 1; }

    curl_args="$(<"${argv_capture}")"
    [[ -n "${curl_args}" ]] || { echo "${label}: curl argv was not captured" >&2; return 1; }
    if [[ "${curl_args}" == *"${secret}"* ]]; then
        echo "${label}: API key leaked into curl argv: ${curl_args}" >&2
        return 1
    fi
    [[ "${curl_args}" == *"@${header_file}"* ]] || {
        echo "${label}: curl argv did not reference the header file: ${curl_args}" >&2
        return 1
    }

    [[ -f "${response_marker}" ]] || { echo "${label}: stub endpoint did not receive the request" >&2; return 1; }
    [[ -f "${output}" ]] || { echo "${label}: helper did not write the response image" >&2; return 1; }
    [[ "$(<"${output}")" == stub-image ]] || { echo "${label}: unexpected image response" >&2; return 1; }
    [[ ! -e "${header_file}" ]] || { echo "${label}: header file survived helper exit" >&2; return 1; }

    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
    server_pid=""
    echo "ok   ${label}: key absent from curl argv; header mode 600; cleanup verified"
}

# One prefix-neutral copy ships to every harness (audit D1).
run_case all-harnesses "${repo_root}/cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh"
echo "PASS: cas-image-generate credential argv and header-file contract verified"
