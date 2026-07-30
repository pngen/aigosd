# AIGOSD 1.0

Supervisor daemon for the AI Governance Operating System.

AIGOSD is the portable, cross-platform supervisor daemon for the AI Governance Operating System.
It loads operator-selected configuration, launches local governance layer binaries, and manages deterministic process lifecycles across named meshes.

AIGOSD requires no installation, system paths, or global directories.
By default, place it alongside its configuration and layer binaries and run it; an operator may
explicitly select another configuration file with `AIGOSD_CONFIG`.

## Architecture
**Preferred layout: place each compiled layer binary directly beside `aigosd`:**
<pre>
/aigosd
/config.yaml
/dio.exe (or `dio` for linux)
/zt-aas.exe
/icae.exe
/poc.exe
/fak.exe
/are.exe
/jib.exe
/icl.exe
/gsas.exe
/able.exe
/iam.exe (if compiled as an extension)
/sck.exe (if compiled as an extension)
</pre>

For backward compatibility, AIGOSD also accepts the original nested layout:
<pre>
/aigosd
/config.yaml
/dio/dio.exe (or `dio/dio` for linux)
/zt-aas/zt-aas.exe
/icae/icae.exe
/poc/poc.exe
/fak/fak.exe
/are/are.exe
/jib/jib.exe
/icl/icl.exe
/gsas/gsas.exe
/able/able.exe
/iam/iam.exe (if compiled as an extension)
/sck/sck.exe (if compiled as an extension)
</pre>

**Linux users**: If you downloaded binaries from the GitHub Releases page (e.g. `dio-linux-x86_64`), rename them to match canonical runtime form (`dio`) before running the daemon.

AIGOSD performs four deterministic steps:
1. Load `config.yaml` from the current working directory, unless `AIGOSD_CONFIG` explicitly selects another file.
2. Load canonical layer definitions baked in at compile time from the aigos library.
3. Verify all ten mandatory Core layer binaries, plus any compiled-in extension binaries, are present before spawning anything.
4. Launch all ten Core layers for each configured Core mesh as supervised subprocesses owned by AIGOSD.

No global install paths are used, and AIGOSD does not create or write global or system
directories. Layer discovery and runtime outputs remain local to the working directory; only an
explicitly overridden configuration input may be read from elsewhere.

## Running
When building from source, compile `aigos` first to bake canonical definitions.
Then compile `aigosd`:

```bash
cargo build --release
```

Place the resulting `aigosd` binary in a working directory containing `config.yaml` and your layer binaries.

If cloning the repo, rename the `aigosd` folder to **`_aigosd`**, and place the compiled `aigosd` binary adjacent to binaries like `gsas`, `dio`, etc.

Then run:
```bash
./aigosd
```

On Windows:
```bash
.\aigosd.exe
```

AIGOSD automatically discovers:
- the local config, unless `AIGOSD_CONFIG` explicitly selects another file
- compiled layer binaries in the same directory, with nested layer folders as a fallback
- canonical Core and extension names embedded at compile-time from the `aigos` world-model registry

## Configuration (`config.yaml`)
A single file placed next to the daemon by default, or selected explicitly with `AIGOSD_CONFIG`.

Example:

```yaml
version: "1.0.0"

meshes:
  mesh1: {}
  mesh2: {}

options:
  logging: structured
  restart: on-failure
  log_file: aigosd.log
```

`version` is optional for backward compatibility. When present it must be `"1.0.0"`.
Unknown configuration fields are rejected so misspelled governance options cannot silently fall
back to defaults. Mesh names must be 1-64 ASCII characters, begin with a letter or digit, and use
only letters, digits, `_`, `.`, or `-`.

`log_file` is optional. It must be a relative path whose existing parent directory resolves inside
the daemon's working directory. Symbolic-link traversal and hard-linked or non-regular targets are
rejected, and the opened file identity is rechecked before any record is written. If the requested
file cannot be opened safely, AIGOSD fails startup; later write failures are reported on standard
error and cause a nonzero shutdown.

**Meshes** are independently managed lifecycle groups of governance processes; they are not
filesystem, network, IPC, identity, or security sandboxes. AIGOSD launches each Core mesh
deterministically and supervises all child processes until shutdown.

`config.yaml` may name Core meshes and set daemon options, but it cannot select a subset of AIGOS Core. Omitting `layers` runs the mandatory ten-layer Core only.

When extensions are unlocked in the `aigos` registry, `config.yaml` may list extension layers:

```yaml
meshes:
  mesh1:
    layers:
      - iam
      - sck
```

This runs all ten Core layers first, then `iam`, then `sck`. Extension order follows the order declared in `config.yaml`.

For backward compatibility, `layers` may list all ten Core layers. AIGOSD still starts Core once and does not double-spawn Core. Mixed full-Core-plus-extension lists also start Core once, then extensions in config order.

## Canonical Layer Names
The daemon recognizes the ten canonical governance layers embedded at compile-time:
- `dio`
- `zt-aas`
- `icae`
- `poc`
- `fak`
- `are`
- `jib`
- `icl`
- `gsas`
- `able`

Layer binaries must use these **exact** names.

## Core And Extensions
AIGOS Core runtime requires all ten Core layers. Core is all-or-none: partial Core execution is rejected before runtime and never used to spawn a mesh.

Extensions are separate from mandatory Core and are additive unlocks. `config.yaml` cannot subtract from Core; it can only add recognized extension layers after Core.

`aigos` is the canonical world-model registry. Adding an extension requires adding its canonical name to `CANONICAL_EXTENSION_LAYERS`, recompiling `aigosd`, and making the extension binary available to the runtime bundle.

When `aigosd` is compiled with extension layers, every compiled-in extension executable must be present in the daemon's working tree before startup proceeds, either beside `aigosd` or in its matching named subfolder. Missing extension executables fail closed before any child process is spawned. Extra unregistered executables are ignored unless they are also recognized by the compiled registry and selected by configuration.

## Deterministic Logging
AIGOSD writes structured or plaintext logs (as selected in options.logging) into the local directory where it is executed.

Structured records are JSON objects. Child output is encoded as data rather than interpolated into
the JSON syntax.

No system log directories are used.

## Portability
AIGOSD requires no global install paths.
It discovers layer binaries and writes runtime outputs in its working directory. Only an
explicitly selected `AIGOSD_CONFIG` input may be read from elsewhere.

AIGOSD is a **portable OS-level supervisor**:
place it beside the Core layer binaries and run it.

## License
AIGOS is also available for enterprise and institutional licensing.
