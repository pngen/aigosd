# AIGOSD 1.0

Supervisor daemon for the AI Governance Operating System.

AIGOSD is the portable, cross-platform supervisor daemon for the AI Governance Operating System.
It loads local configuration, launches governance layer binaries, and manages deterministic process lifecycles across named meshes.

AIGOSD is self-contained: no installation, no system paths, no global directories.
Place it anywhere, alongside its configuration and layer binaries, and run it.

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
</pre>

**Linux users**: If you downloaded binaries from the GitHub Releases page (e.g. `dio-linux-x86_64`), rename them to match canonical runtime form (`dio`) before running the daemon.

AIGOSD performs four deterministic steps:
1. Load `config.yaml` from the current working directory.
2. Load canonical layer definitions baked in at compile time from the aigos library.
3. Verify all ten mandatory Core layer binaries are present before spawning anything.
4. Launch all ten Core layers for each configured Core mesh as supervised subprocesses owned by AIGOSD.

No global install paths are used.
No system directories are touched.
All behavior is local to the folder where you run the daemon.

## Running
Compile `aigos` first to bake canonical definitions.
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
- the local config
- the compiled layer binaries in the same directory, with nested layer folders as a fallback
- canonical Core and extension names embedded at compile-time from the `aigos` world-model registry

## Configuration (`config.yaml`)
A single file placed next to the daemon.

Example:

```yaml
version: "1.0.0"

meshes:
  mesh1: {}
  mesh2: {}

options:
  logging: structured
  restart: on-failure
```

**Meshes** are isolated groups of governance processes.
AIGOSD launches each Core mesh independently, deterministically, and supervises all child processes until shutdown.

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

## Deterministic Logging
AIGOSD writes structured or plaintext logs (as selected in options.logging) into the local directory where it is executed.

No system log directories are used.

## Portability
AIGOSD requires no global install paths.
It runs entirely from its working directory.

AIGOSD is a **portable OS-level supervisor**:
place it beside the Core layer binaries and run it.

## License
AIGOS is also available for enterprise and institutional licensing.
