# AIGOSD 1.0

Supervisor daemon for the AI Governance Operating System.

AIGOSD is the portable, cross-platform supervisor daemon for the AI Governance Operating System.
It loads local configuration, launches governance layer binaries, and manages deterministic process lifecycles across named meshes.

AIGOSD is self-contained: no installation, no system paths, no global directories.
Place it anywhere, alongside its configuration and layer binaries, and run it.

## Architecture
**Each layer must have its own directory, named exactly after the layer, containing its compiled binary:**
<pre>
/aigosd
/config.yaml
/dio/dio.exe (or `dio` for linux)
/zt-aas/zt-aas.exe
/icae/icae.exe
/poc/poc.exe
...
</pre>

**Linux users**: If you downloaded binaries from the GitHub Releases page (e.g. `dio-linux-x86_64`), simply rename them to match canonical runtime form (`dio/dio`) before running the daemon.

AIGOSD performs three deterministic steps:
1. Load `config.yaml` from the current working directory.
2. Load canonical layer definitions baked in at compile time from the aigos library.
3. Launch each mesh’s process set as independent supervised subprocesses.

No global install paths are used.
No system directories are touched.
All behavior is local to the folder where you run the daemon.

## Running
Compile `aigos` first to bake canonical definitions.
Then compile `aigosd`:

```bash
cargo build --release
```

Place the resulting `aigosd` binary in a working directory containing `config.yaml` and your layer folders.

If cloning the repo, rename the `aigosd` folder to **`_aigosd`**, and place the compiled `aigosd` binary adjacent to the folders like `gsas/gsas`, `dio/dio`, etc.

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
- the compiled layer binaries in the same directory
- canonical names and mappings embedded at compile-time from the `aigos` library

## Configuration (`config.yaml`)
A single file placed next to the daemon.

Example:

```yaml
version: "1.0.0"

meshes:
  mesh1:
    layers:
      - dio
      - zt-aas
      - icae
      - poc

  mesh2:
    layers:
      - fak
      - are

options:
  logging: structured
  restart: on-failure
```

**Meshes** are isolated groups of governance processes.
AIGOSD launches each mesh independently, deterministically, and supervises all child processes until shutdown.

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

## Deterministic Logging
AIGOSD writes structured or plaintext logs (as selected in options.logging) into the local directory where it is executed.

No system log directories are used.

## Portability
AIGOSD requires no global install paths.
It runs entirely from its working directory.

AIGOSD is a **portable OS-level supervisor**:
place it beside your folders with the binaries in them and run it.

## License
AIGOS is also available for enterprise and institutional licensing.