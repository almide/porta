# Pin executable artifacts and delegated policies

An agent configuration can bind local WASM modules and delegated configuration
files to SHA-256 digests. Porta verifies the exact bytes before compiling a module
or parsing a delegated policy. A mismatch prevents launch of the entire team,
before model requests or tool effects. Resume and offline replay perform the same
checks when loading their configuration tree.

```toml
version = 1
require_artifact_hashes = true

[agent]
wasm = "agent.wasm"
sha256 = "<64 lowercase hexadecimal characters>"

[model]
endpoint = "https://model.example/v1/chat/completions"
name = "configured-model"

[[tools]]
name = "read_file"
wasm = "tools.wasm"
sha256 = "<64 lowercase hexadecimal characters>"
mounts = [{host="workspace", guest="."}]

[[before_tool_checks]]
name = "prerequisite"
wasm = "before-tool.wasm"
sha256 = "<64 lowercase hexadecimal characters>"
parameters = {tool="write_file", requires="run_tests"}

[[completion_checks]]
name = "artifact"
wasm = "check.wasm"
sha256 = "<64 lowercase hexadecimal characters>"
parameters = {path="result.json", expected={value=42}}
mounts = [{host="workspace", guest="."}]

[[agents]]
name = "worker"
config = "worker.toml"
sha256 = "<SHA-256 of the exact worker.toml bytes>"
```

Replace every placeholder with a reviewed digest. These are identity settings;
the referenced files, grants and tools still need to be configured for the task.
For a file already obtained from a trusted source:

```bash
python3 -c 'import hashlib,pathlib,sys; print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())' agent.wasm
```

## Required and optional pins

`require_artifact_hashes` defaults to false. A provided `sha256` is always checked,
including in optional mode. Empty, non-hexadecimal, uppercase or incorrectly sized
digests are errors. The format is exactly 64 lowercase hexadecimal characters,
without a `sha256:` prefix. Existing configurations that omit pins remain usable.

Strict mode requires pins for the agent, every local tool, both kinds of verifier,
and every delegated configuration file. The requirement propagates through the
entire configured team, including unused delegates. A descendant's explicit
`require_artifact_hashes = false` cannot weaken its ancestor's requirement. A
child can independently enable strict mode for its own modules and descendants.

The root configuration is the trust anchor supplied by the operator; it does not
contain a self-hash. Its delegated configuration pins bind exact source bytes,
including comments and line endings. Those pinned configurations in turn bind
their module digests, grants and descendants. Changing a delegated policy requires
reviewing and updating the parent pin and any pins above it.

## Execution and journal behavior

Modules are compiled from the same byte buffer that was hashed, without reopening
the module between verification and compilation. Delegated source is likewise
parsed from the checked string. Modules and the complete policy tree are frozen
before any agent runs. Replacing an artifact during a run cannot change the
already-loaded module; the next launch rejects the replacement if its digest
no longer matches. A matching digest does not bypass WASM validity checks,
unsupported-import rejection, capability enforcement or runtime budgets.

Journal fingerprints already include configuration source and actual module
hashes. Adding or changing pins changes configuration identity, so existing
journals still require their original configuration and artifacts. Pin loading
does not alter broker operation/replay semantics; broker protocol remains 5.

## Scope of the guarantee

Pins establish content identity relative to trusted configuration. They do not
authenticate a publisher or prove that a module is safe. Protect the root policy
and verify distribution provenance independently. Signed manifests, key trust,
revocation, a content-addressed registry and portable bundle packaging remain
separate delivery work.

Mutable mounted data, model responses and remote MCP server implementations are
outside these executable-artifact pins. Filesystem and network grants still
govern access, and task correctness still requires appropriate pre-tool and
completion policies. This feature does not snapshot external resources or prevent
other host processes from editing them.

```bash
python3 scripts/artifact_pins_integration.py target/porta examples/chat-agent/agent.wasm examples/demo-agent/agent.wasm
```

The integration suite covers valid pinned teams, all local module kinds, missing
and malformed pins, descendant enforcement, policy/source changes before model
requests, offline replay, invalid WASM with a matching digest, and file replacement
after launch while the original compiled tool executes.
