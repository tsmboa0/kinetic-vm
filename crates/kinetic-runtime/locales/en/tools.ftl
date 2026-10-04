# English tool descriptions (default locale, embedded at compile time)
#
# Keys follow the pattern: tool-{name-with-hyphens}
# e.g. "file_read" → "tool-file-read", "web_search_tool" → "tool-web-search-tool"
#
# Literal { and } in values must be escaped as {"{"}  and  {"}"} respectively.

tool-backup = Create, list, verify, and restore shared data directory backups




tool-channel-room = Create rooms and invite users through an active channel. Provide a channel key such as 'matrix.default', action 'create_room' or 'invite_user', and the action-specific room fields.
tool-channel-room-param-action = Room-management action to perform.
tool-channel-room-param-channel = Active channel key such as 'matrix.default'.
tool-channel-room-param-name = Optional room name for create_room.
tool-channel-room-param-topic = Optional room topic for create_room.
tool-channel-room-param-invites = Optional user IDs to invite while creating the room.
tool-channel-room-param-visibility = Optional room visibility for create_room.
tool-channel-room-param-encryption = Whether to request room encryption during create_room.
tool-channel-room-param-room-id = Existing room ID for invite_user.
tool-channel-room-param-user-id = User ID to invite for invite_user.
tool-channel-room-error-security = Action blocked: { $err }
tool-channel-room-error-invalid-action = Invalid action '{ $action }': must be 'create_room' or 'invite_user'.
tool-channel-room-error-not-initialized = No channels available yet (channels not initialized).
tool-channel-room-error-channel-not-found = Channel '{ $channel }' not found. Available channels: { $available }
tool-channel-room-error-create-failed = Failed to create room: { $err }
tool-channel-room-error-invite-failed = Failed to invite user: { $err }
tool-channel-room-error-invites-array = 'invites' must be an array of strings.
tool-channel-room-error-invites-item = 'invites' must be an array of non-empty strings.
tool-channel-room-error-invalid-visibility = Invalid room visibility: { $err }
tool-channel-room-error-missing-param = Missing '{ $param }' parameter.
tool-channel-room-error-string-param = '{ $param }' must be a string.
tool-channel-room-error-bool-param = '{ $param }' must be a boolean.




tool-content-search = Search file contents by regex pattern within the workspace. Supports ripgrep (rg) with grep or internal fallback. Output modes: 'content' (matching lines with context), 'files_with_matches' (file paths only), 'count' (match counts per file). Example: pattern='fn main', include='*.rs', output_mode='content'.

tool-cron-add = Create a scheduled cron job (shell or agent) with cron/at/every schedules. Use job_type='agent' with a prompt to run the AI agent on schedule. To deliver output to a channel (Discord, Telegram, Slack, Mattermost, Matrix), set delivery={"{"}"mode":"announce","channel":"discord","to":"<channel_id_or_chat_id>"{"}"}. This is the preferred tool for sending scheduled/delayed messages to users via channels.

tool-cron-list = List all scheduled cron jobs

tool-cron-remove = Remove a cron job by id

tool-cron-run = Force-run a cron job immediately and record run history

tool-cron-runs = List recent run history for a cron job

tool-cron-update = Patch an existing cron job (schedule, command, prompt, enabled, delivery, model, etc.)

tool-data-management = Shared data directory retention preview and storage statistics

tool-delegate = Delegate a subtask to a specialized agent. Use when: a task benefits from a different model (e.g. fast summarization, deep reasoning, code generation). The sub-agent runs a single prompt by default; with agentic=true it can iterate with a filtered tool-call loop.

tool-file-edit = Edit a file by replacing an exact string match with new content

tool-file-download = Download a file from the configured remote endpoint and write it to the agent's workspace. Supply the identifier of the document to fetch and a workspace-relative destination path; the endpoint URL is fixed by host config and is never model-controlled. Bytes are streamed straight to disk and are not loaded into model context. Returns the HTTP status, the number of bytes written, and the destination path.
tool-file-download-param-document-id = Identifier of the document to fetch from the configured endpoint.
tool-file-download-param-dest-path = Workspace-relative path to write the file to. The parent directory must already exist.
tool-file-download-error-disabled = file_download is disabled: [file_download].url is not configured
tool-file-download-error-read-only = Action blocked: autonomy is read-only
tool-file-download-error-rate-limited-hour = Rate limit exceeded: too many actions in the last hour
tool-file-download-error-rate-limited-budget = Rate limit exceeded: action budget exhausted
tool-file-download-error-missing-document-id = Missing 'document_id' parameter
tool-file-download-error-missing-dest-path = Missing 'dest_path' parameter
tool-file-download-error-invalid-file-name = Invalid dest_path '{ $dest_path }': must end in a concrete file name
tool-file-download-error-no-parent = Invalid dest_path '{ $dest_path }': has no parent directory
tool-file-download-error-resolve-dir = Cannot resolve destination directory for '{ $dest_path }': { $err }
tool-file-download-error-bad-scheme = file_download endpoint URL scheme '{ $scheme }' is not supported; only http:// and https:// are allowed
tool-file-download-error-invalid-url = file_download endpoint URL is invalid: { $err }
tool-file-download-error-private-host = file_download endpoint host '{ $host }' is loopback / private / link-local. To allow this host, add it (or "*") to { $config_key } in config.toml
tool-file-download-error-metadata-endpoint = file_download endpoint host '{ $host }' resolved to cloud metadata or credential-delivery address { $ip }, which cannot be enabled by file_download.allowed_private_hosts
tool-file-download-error-invalid-nat64-prefix = file_download config '{ $config_key }' contains malformed entry '{ $prefix }': fix or remove it, then retry (a network-specific NAT64 prefix must be an IPv6 CIDR with length 32, 40, 48, 56, 64, or 96)
tool-file-download-error-client-build = Failed to build download client: { $err }
tool-file-download-error-request = Download request failed: { $err }
tool-file-download-error-status = Download endpoint returned status { $status }
tool-file-download-error-too-large-reported = Download too large: endpoint reports { $len } bytes (limit: { $limit } bytes)
tool-file-download-error-too-large-stream = Download too large: exceeded limit of { $limit } bytes
tool-file-download-error-temp-create = Failed to create temporary download file: { $err }
tool-file-download-error-read-body = Failed while reading response body: { $err }
tool-file-download-error-write-body = Failed while writing downloaded bytes: { $err }
tool-file-download-error-flush = Failed to flush downloaded file: { $err }
tool-file-download-error-move = Failed to move downloaded file into place: { $err }
tool-file-download-success = Downloaded { $written } bytes to { $dest_path } ({ $status })

tool-file-read = Read file contents with line numbers. Supports partial reading via offset and limit. Binary and image files are rejected (use the image_info tool for images). Set encoding="base64" to return raw bytes base64-encoded (for binary files such as .pdf/.xlsx/.docx); offset/limit are ignored in that mode.

tool-file-write = Write contents to a file in the workspace
tool-file-write-error-path-blocked = Path blocked by security policy: '{ $path }'
tool-file-write-error-missing-parent = Invalid path: missing parent directory
tool-file-write-error-no-existing-parent = Failed to resolve an existing parent directory
tool-file-write-error-parent-binding = Failed to resolve the file-write parent path: '{ $path }'
tool-file-write-error-missing-name = Invalid path: missing file name
tool-file-write-error-runtime-config = Runtime configuration files cannot be changed with file_write: '{ $path }'
tool-file-write-error-capability-binding = Failed to bind the file-write parent to an authorized directory
tool-file-write-error-symlink = Refusing to write through a symlink: '{ $path }'
tool-filesystem-boundary-error-symlink = Refusing to follow a symlink at '{ $path }'
tool-filesystem-boundary-error-contained = Path must be relative and contained: '{ $path }'
tool-filesystem-boundary-error-not-directory = Path component is not a directory: '{ $path }'
tool-filesystem-boundary-error-not-regular = Refusing to open a non-regular file: '{ $path }'
tool-data-management-error-purge-disabled = Confirmed purge is unavailable; use dry_run to preview eligible files
tool-data-management-error-read-blocked = Shared data path is not readable under the security policy: '{ $path }'
tool-backup-error-max-keep = Backup retention max_keep must be at least 1
tool-backup-error-action-blocked = Backup mutation is blocked by the security policy
tool-backup-error-source-overlap = Backup source cannot contain the backup output directory: '{ $path }'
tool-backup-error-rotation-platform = Backup rotation is unavailable on this platform because recursive deletion cannot preserve the verified directory-handle boundary
tool-backup-error-not-found = Backup not found: '{ $name }'
tool-backup-error-integrity = Integrity check failed
tool-backup-error-non-utf8 = Backup contains a non-UTF-8 entry name
tool-backup-error-contained = Backup path must stay within the shared data directory: '{ $path }'
tool-backup-error-invalid-name = Invalid backup name: '{ $name }'
tool-backup-error-symlink = Refusing to traverse a symlink in backup data: '{ $path }'
tool-backup-error-not-directory = Backup path is not a directory: '{ $path }'
tool-backup-error-is-directory = Backup file destination is a directory: '{ $path }'
tool-backup-error-special-file = Refusing to traverse a special file in backup data: '{ $path }'
tool-backup-error-read-blocked = Shared data path is not readable under the security policy: '{ $path }'
tool-backup-error-write-blocked = Backup destination is not writable under the security policy: '{ $path }'



tool-glob-search = Search for files matching a glob pattern within the workspace. Returns a sorted list of matching file paths relative to the workspace root. Examples: '**/*.rs' (all Rust files), 'src/**/mod.rs' (all mod.rs in src).


tool-hardware-board-info = Return full board info (chip, architecture, memory map) for connected hardware. Use when: user asks for 'board info', 'what board do I have', 'connected hardware', 'chip info', 'what hardware', or 'memory map'.

tool-hardware-memory-map = Return the memory map (flash and RAM address ranges) for connected hardware. Use when: user asks for 'upper and lower memory addresses', 'memory map', 'address space', or 'readable addresses'. Returns flash/RAM ranges from datasheets.

tool-hardware-memory-read = Read actual memory/register values from Nucleo via USB. Use when: user asks to 'read register values', 'read memory at address', 'dump memory', 'lower memory 0-126', or 'give address and value'. Returns hex dump. Requires Nucleo connected via USB and probe feature. Params: address (hex, e.g. 0x20000000 for RAM start), length (bytes, default 128).

tool-http-request = Make HTTP requests to external APIs. Supports GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS methods. Security constraints: allowlist-only domains, no local/private hosts, configurable timeout and response size limits.

tool-image-info = Read image file metadata (format, dimensions, size) and optionally return base64-encoded data.


tool-knowledge = Manage a knowledge graph of architecture decisions, solution patterns, lessons learned, experts, and relationship links.



tool-memory-forget = Remove a memory by key. Use to delete outdated facts or sensitive data. Returns whether the memory was found and removed.

tool-memory-recall = Search long-term memory for relevant facts, preferences, or context. Returns scored results ranked by relevance. Omit the query or pass bare * to return recent memories.

tool-memory-store = Store a fact, preference, or note in long-term memory. Use category 'core' for permanent facts, 'daily' for session notes, 'conversation' for chat context, or a custom category name.


tool-model-routing-config = Manage default model settings, scenario-based provider/model routes, classification rules, and aliased agent profiles




tool-proxy-config = Manage KineticVM proxy settings (scope: environment | kinetic | services), including runtime and process env application


tool-schedule = Manage scheduled shell-only tasks. Actions: create/add/once/list/get/cancel/remove/pause/resume. WARNING: This tool creates shell jobs whose output is only logged, NOT delivered to any channel. To send a scheduled message to Discord/Telegram/Slack/Matrix, use the cron_add tool with job_type='agent' and a delivery config like {"{"}"mode":"announce","channel":"discord","to":"<channel_id>"{"}"}.

tool-sessions-history-header = Session '{ $session_id }': showing { $shown }/{ $total } messages
tool-sessions-send-error-acp-unsupported = { $tool } does not support { $channel } sessions because durable transcript writes do not deliver messages to the live { $product } session.
tool-sessions-current-channel = Channel: { $channel }



tool-shell = Execute a shell command in the workspace directory

tool-sop-advance = Report the result of the current SOP step and advance to the next step. Provide the run_id, whether the step succeeded or failed, and a brief output summary.

tool-sop-approve = Approve a pending SOP step that is waiting for operator approval. Returns the step instruction to execute. Use sop_status to see which runs are waiting.

tool-sop-execute = Manually trigger a Standard Operating Procedure (SOP) by name. Returns the run ID and first step instruction. Use sop_list to see available SOPs.

tool-sop-list = List all loaded Standard Operating Procedures (SOPs) with their triggers, priority, step count, and active run count. Optionally filter by name or priority.

tool-sop-status = Query SOP execution status. Provide run_id for a specific run, or sop_name to list runs for that SOP. With no arguments, shows all active runs.

tool-tool-search = Fetch full schema definitions for deferred MCP tools so they can be called. Use "select:name1,name2" for exact match or keywords to search.

tool-web-fetch = Fetch a web page and return its content as clean plain text. HTML pages are automatically converted to readable text. JSON and plain text responses are returned as-is. Only GET requests; follows redirects. Security: allowlist-only domains, no local/private hosts.

tool-web-search-tool = Search the web for information. Returns relevant search results with titles, URLs, and descriptions. Use this to find current information, news, or research topics.
tool-web-search-tool-error-duckduckgo-blocked = DuckDuckGo is rate-limiting this machine. Do not retry or rephrase the search; wait a few minutes, fetch known URLs directly with web_fetch, or configure SearXNG, Brave, or Tavily as the web_search provider.
tool-web-search-tool-error-searxng-not-configured = SearXNG instance URL not configured. Set [web_search] searxng_instance_url in config.toml, or override it with the KINETIC_web_search__searxng_instance_url environment variable.
tool-web-search-tool-note-truncated-results = (further results omitted)

tool-workspace = Manage multi-client workspaces. Subcommands: list, switch, create, info, export. Each workspace provides isolated memory, audit, secrets, and tool restrictions.


tool-a2a-discover = List available remote A2A peer agents and their advertised capabilities. Call with no peer to list all configured peers, or a specific peer to fetch its Agent Card (name, description, skills). Use before a2a_send to find the right peer and agent for a task.
tool-a2a-discover-desc-peer = Peer name to fetch the Agent Card for. Omit to list all configured peers.
tool-a2a-discover-desc-filter-tags = Optional tags to filter peers by (e.g. ["production"]).
tool-a2a-send = Delegate a task to a remote A2A peer agent and wait for the result. Returns a Task with a task_id, state, and artifacts (the peer's reply, fenced as untrusted-external). If the state is non-terminal (working/input-required), poll with a2a_get_task or cancel with a2a_cancel. The message is sent as-is. This is an Act operation that requires approval by default (not in auto_approve) unless the operator explicitly opts in via risk_profiles.<name>.auto_approve.
tool-a2a-send-desc-peer = Configured peer name to send the task to.
tool-a2a-send-desc-agent = Target route identity on the peer: the agent alias ({"{"}alias{"}"} in /a2a/{"{"}alias{"}"}) or, when the card shares a URL across tenants, the tenant of the interface to reach.
tool-a2a-send-desc-message = The task prompt to send to the peer agent.
tool-a2a-send-desc-return-immediately = Default false (block for a terminal state). Set true to return immediately with a non-terminal (working/input-required) task for polling.
tool-a2a-send-desc-context-id = Optional context ID for multi-turn continuation (from a prior send's response).
tool-a2a-send-desc-task-id = Optional task ID for continuing an existing task (e.g. after INPUT_REQUIRED).
tool-a2a-get-task = Retrieve the current state and artifacts of an in-flight A2A task on a peer. Use to poll a task that a2a_send returned in a non-terminal state (working/input-required).
tool-a2a-get-task-desc-peer = Configured peer name hosting the task.
tool-a2a-get-task-desc-task-id = The task id returned by a2a_send.
tool-a2a-get-task-desc-agent = Optional agent alias or tenant that created the task (from a2a_send). Helps route the poll to the correct interface when discovery is re-run (the cached route is used first).
tool-a2a-cancel = Cancel an in-flight A2A task on a peer. Returns the updated Task (typically state=canceled, though the spec does not guarantee it).
tool-a2a-cancel-desc-peer = Configured peer name hosting the task.
tool-a2a-cancel-desc-task-id = The task id to cancel.
tool-a2a-cancel-desc-agent = Optional agent alias or tenant that created the task (from a2a_send). Helps route the cancel to the correct interface when discovery is re-run (the cached route is used first).
