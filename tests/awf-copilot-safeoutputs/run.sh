#!/usr/bin/env bash
set -Eeuo pipefail

umask 077

: "${ADO_AW_BIN:?ADO_AW_BIN is required}"
: "${AWF_BIN:?AWF_BIN is required}"
: "${COPILOT_BIN:?COPILOT_BIN is required}"
: "${AWF_VERSION:?AWF_VERSION is required}"
: "${MCPG_VERSION:?MCPG_VERSION is required}"
: "${ADO_AW_COPILOT_CLI_ARTIFACT_DIR:?ADO_AW_COPILOT_CLI_ARTIFACT_DIR is required}"
: "${COPILOT_GITHUB_TOKEN:?COPILOT_GITHUB_TOKEN is required}"

readonly CONTRACT_CONTEXT="awf-copilot-safeoutputs-contract"
readonly MCP_GATEWAY_PORT=8080
readonly MCP_GATEWAY_CONTAINER="awmg-mcpg"
readonly MCPG_IMAGE="ghcr.io/github/gh-aw-mcpg:v${MCPG_VERSION}"
readonly SAFEOUTPUTS_IMAGE="ghcr.io/github/gh-aw-firewall/agent:${AWF_VERSION}"
readonly ARTIFACT_DIR="${ADO_AW_COPILOT_CLI_ARTIFACT_DIR}"

RUNTIME_DIR="$(mktemp -d /tmp/ado-aw-awf-contract.XXXXXX)"
SAFE_OUTPUTS_DIR="$(mktemp -d /tmp/ado-aw-safeoutputs.XXXXXX)"
TOOLS_DIR="/tmp/awf-tools"
MCPG_PID=""

mkdir -p "${ARTIFACT_DIR}" "${SAFE_OUTPUTS_DIR}" "${TOOLS_DIR}"

cleanup() {
  local status=$?
  set +e
  if [[ -n "${MCPG_PID}" ]]; then
    docker rm -f "${MCP_GATEWAY_CONTAINER}" >/dev/null 2>&1 || true
    kill "${MCPG_PID}" >/dev/null 2>&1 || true
    wait "${MCPG_PID}" >/dev/null 2>&1 || true
  fi
  docker ps -a > "${ARTIFACT_DIR}/docker-ps.txt" 2>&1 || true
  if [[ -f "${SAFE_OUTPUTS_DIR}/safe_outputs.ndjson" ]]; then
    cp "${SAFE_OUTPUTS_DIR}/safe_outputs.ndjson" "${ARTIFACT_DIR}/safe_outputs.ndjson"
  fi
  rm -rf "${RUNTIME_DIR}" "${SAFE_OUTPUTS_DIR}"
  return "${status}"
}
trap cleanup EXIT

wait_for_http() {
  local label=$1
  local url=$2
  local pid=$3
  local log_path=$4

  for _ in $(seq 1 60); do
    if curl --noproxy '*' -fsS "${url}" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "${pid}" >/dev/null 2>&1; then
      echo "${label} exited before becoming ready" >&2
      cat "${log_path}" >&2 || true
      return 1
    fi
    sleep 1
  done

  echo "${label} did not become ready within 60 seconds" >&2
  cat "${log_path}" >&2 || true
  return 1
}

for binary in "${ADO_AW_BIN}" "${AWF_BIN}" "${COPILOT_BIN}"; do
  [[ -x "${binary}" ]] || {
    echo "Required binary is not executable: ${binary}" >&2
    exit 1
  }
done

MCP_GATEWAY_API_KEY="$(openssl rand -base64 45 | tr -d '/+=')"
install -m 0755 "${ADO_AW_BIN}" "${TOOLS_DIR}/ado-aw"

for image in squid agent api-proxy; do
  docker pull "ghcr.io/github/gh-aw-firewall/${image}:${AWF_VERSION}"
done
docker pull "${MCPG_IMAGE}"

jq -n \
  --arg safeoutputs_image "${SAFEOUTPUTS_IMAGE}" \
  --arg ado_aw_bin "${TOOLS_DIR}/ado-aw" \
  --arg runtime_dir "${RUNTIME_DIR}" \
  --arg safeoutputs_dir "${SAFE_OUTPUTS_DIR}" \
  --arg runner_uid "$(id -u)" \
  --arg runner_gid "$(id -g)" \
  --arg gateway_key "${MCP_GATEWAY_API_KEY}" \
  --arg gateway_domain "${MCP_GATEWAY_CONTAINER}" \
  --argjson gateway_port "${MCP_GATEWAY_PORT}" \
  '{
    mcpServers: {
      safeoutputs: {
        type: "stdio",
        container: $safeoutputs_image,
        entrypoint: "/usr/local/bin/ado-aw",
        entrypointArgs: [
          "mcp", "--enabled-tools", "noop", "/safeoutputs", $runtime_dir
        ],
        mounts: [
          ($ado_aw_bin + ":/usr/local/bin/ado-aw:ro"),
          ($runtime_dir + ":" + $runtime_dir + ":rw"),
          ($safeoutputs_dir + ":/safeoutputs:rw")
        ],
        args: [
          "--network", "none",
          "--user", ($runner_uid + ":" + $runner_gid),
          "--cap-drop", "ALL",
          "--security-opt", "no-new-privileges",
          "--read-only",
          "--tmpfs", "/tmp:rw,nosuid,nodev,noexec",
          "--pids-limit", "256",
          "-w", $runtime_dir
        ],
        env: {HOME: "/tmp"}
      }
    },
    gateway: {
      port: $gateway_port,
      domain: $gateway_domain,
      apiKey: $gateway_key,
      payloadDir: "/tmp/gh-aw/mcp-payloads"
    }
  }' >"${RUNTIME_DIR}/mcpg-config.json"

docker rm -f "${MCP_GATEWAY_CONTAINER}" >/dev/null 2>&1 || true

docker run -i --rm \
  --name "${MCP_GATEWAY_CONTAINER}" \
  --network bridge \
  -p "127.0.0.1:${MCP_GATEWAY_PORT}:${MCP_GATEWAY_PORT}" \
  --entrypoint /app/awmg \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -e "MCP_GATEWAY_PORT=${MCP_GATEWAY_PORT}" \
  -e "MCP_GATEWAY_DOMAIN=${MCP_GATEWAY_CONTAINER}" \
  -e "MCP_GATEWAY_API_KEY=${MCP_GATEWAY_API_KEY}" \
  "${MCPG_IMAGE}" \
  --routed \
  --listen "0.0.0.0:${MCP_GATEWAY_PORT}" \
  --config-stdin \
  --log-dir /tmp/gh-aw/mcp-logs \
  <"${RUNTIME_DIR}/mcpg-config.json" \
  >"${RUNTIME_DIR}/gateway-output.json" \
  2>"${ARTIFACT_DIR}/mcpg.stderr.log" &
MCPG_PID=$!

wait_for_http \
  "MCPG" \
  "http://127.0.0.1:${MCP_GATEWAY_PORT}/health" \
  "${MCPG_PID}" \
  "${ARTIFACT_DIR}/mcpg.stderr.log"

for _ in $(seq 1 30); do
  if [[ -s "${RUNTIME_DIR}/gateway-output.json" ]] &&
    jq -e '.mcpServers.safeoutputs.url' "${RUNTIME_DIR}/gateway-output.json" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
jq -e '.mcpServers.safeoutputs.url' "${RUNTIME_DIR}/gateway-output.json" >/dev/null

MCPG_PROBE_STATUS="$(
  curl --noproxy '*' -sS \
    -D "${ARTIFACT_DIR}/mcpg-safeoutputs-probe-headers.txt" \
    -o "${ARTIFACT_DIR}/mcpg-safeoutputs-probe.json" \
    -w '%{http_code}' \
    -X POST "http://127.0.0.1:${MCP_GATEWAY_PORT}/mcp/safeoutputs" \
    -H "Authorization: ${MCP_GATEWAY_API_KEY}" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"ado-aw-contract-probe","version":"1.0"}}}'
)"
if [[ ! "${MCPG_PROBE_STATUS}" =~ ^2 ]]; then
  echo "MCPG SafeOutputs probe failed with HTTP ${MCPG_PROBE_STATUS}" >&2
  cat "${ARTIFACT_DIR}/mcpg-safeoutputs-probe.json" >&2 || true
  exit 1
fi

MCPG_PROBE_SESSION="$(
  grep -i '^mcp-session-id:' "${ARTIFACT_DIR}/mcpg-safeoutputs-probe-headers.txt" |
    tr -d '\r' |
    awk '{print $2}'
)"
if [[ -z "${MCPG_PROBE_SESSION}" ]]; then
  echo "MCPG SafeOutputs probe did not return a session ID" >&2
  exit 1
fi

MCPG_TOOLS_STATUS="$(
  curl --noproxy '*' -sS \
    -o "${ARTIFACT_DIR}/mcpg-safeoutputs-tools.json" \
    -w '%{http_code}' \
    -X POST "http://127.0.0.1:${MCP_GATEWAY_PORT}/mcp/safeoutputs" \
    -H "Authorization: ${MCP_GATEWAY_API_KEY}" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -H "Mcp-Session-Id: ${MCPG_PROBE_SESSION}" \
    -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
)"
if [[ ! "${MCPG_TOOLS_STATUS}" =~ ^2 ]] ||
  ! grep -q '"name":"noop"' "${ARTIFACT_DIR}/mcpg-safeoutputs-tools.json"; then
  echo "MCPG SafeOutputs tools/list did not expose noop (HTTP ${MCPG_TOOLS_STATUS})" >&2
  cat "${ARTIFACT_DIR}/mcpg-safeoutputs-tools.json" >&2 || true
  exit 1
fi

jq \
  --arg prefix "http://${MCP_GATEWAY_CONTAINER}:${MCP_GATEWAY_PORT}" \
  '.mcpServers |= (
    to_entries
    | sort_by(.key)
    | map(
        .value.url |= sub("^http://[^/]+/"; "\($prefix)/")
        | .value.tools = ["*"]
        | .value.isDefaultServer = true
      )
    | from_entries
  )' \
  "${RUNTIME_DIR}/gateway-output.json" \
  >"${TOOLS_DIR}/mcp-config.json"
chmod 600 "${TOOLS_DIR}/mcp-config.json"

install -m 0755 "${COPILOT_BIN}" "${TOOLS_DIR}/copilot"
cat >"${TOOLS_DIR}/agent-prompt.md" <<EOF
Call the noop tool exactly once with context "${CONTRACT_CONTEXT}".
Do not call any other tool. Stop immediately after the tool call.
EOF
chmod 600 "${TOOLS_DIR}/agent-prompt.md"

readonly ALLOWED_DOMAINS="api.business.githubcopilot.com,api.enterprise.githubcopilot.com,api.github.com,api.githubcopilot.com,api.individual.githubcopilot.com,config.edge.skype.com,copilot-proxy.githubusercontent.com,github.com,telemetry.enterprise.githubcopilot.com,*.copilot.github.com,*.githubcopilot.com"
# shellcheck disable=SC2016 # AWF expands the engine command inside the sandbox.
readonly ENGINE_RUN='export NO_PROXY="${NO_PROXY:+$NO_PROXY,}awmg-mcpg"; export no_proxy="$NO_PROXY"; /tmp/awf-tools/copilot --prompt "$(cat /tmp/awf-tools/agent-prompt.md)" --additional-mcp-config @/tmp/awf-tools/mcp-config.json --model gpt-5-mini --disable-builtin-mcps --no-ask-user --allow-all-tools --allow-tool safeoutputs --allow-all-paths'

set +e
"${AWF_BIN}" \
  --allow-domains "${ALLOWED_DOMAINS}" \
  --network-isolation \
  --topology-attach "${MCP_GATEWAY_CONTAINER}" \
  --image-tag "${AWF_VERSION}" \
  --skip-pull \
  --env-all \
  --container-workdir "${RUNTIME_DIR}" \
  --audit-dir "${ARTIFACT_DIR}/awf-audit" \
  --diagnostic-logs \
  -- "${ENGINE_RUN}" \
  2>&1 | tee "${ARTIFACT_DIR}/awf-copilot.log"
AWF_STATUS=${PIPESTATUS[0]}
set -e

if [[ "${AWF_STATUS}" -ne 0 ]]; then
  echo "AWF Copilot run failed with exit code ${AWF_STATUS}" >&2
  exit "${AWF_STATUS}"
fi

NDJSON_PATH="${SAFE_OUTPUTS_DIR}/safe_outputs.ndjson"
for _ in $(seq 1 30); do
  [[ -s "${NDJSON_PATH}" ]] && break
  sleep 1
done
[[ -s "${NDJSON_PATH}" ]] || {
  echo "SafeOutputs did not write safe_outputs.ndjson" >&2
  exit 1
}

TOTAL_ENTRIES="$(grep -cve '^[[:space:]]*$' "${NDJSON_PATH}")"
MATCHING_ENTRIES="$(
  jq -s \
    --arg context "${CONTRACT_CONTEXT}" \
    '[.[] | select(.name == "noop" and .context == $context)] | length' \
    "${NDJSON_PATH}"
)"

if [[ "${TOTAL_ENTRIES}" -ne 1 || "${MATCHING_ENTRIES}" -ne 1 ]]; then
  echo "Expected exactly one matching noop proposal; got ${TOTAL_ENTRIES} total and ${MATCHING_ENTRIES} matching" >&2
  cat "${NDJSON_PATH}" >&2
  exit 1
fi

echo "AWF Copilot SafeOutputs contract passed"
