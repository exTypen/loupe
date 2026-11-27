# Flows fixture

A deterministic mock backend for trying Loupe's user flows by hand or from scripts, without real accounts or
real data: three repositories (`acme/app`, `acme/lib`, `acme/infra`) with worktrees, 38 pull requests, CI runs,
LFS files, submodules and conflict states, served by a mock Forgejo.

```sh
scripts/flows/up.sh /tmp/flows            # builds the fixture (once) and starts the mock forge on :8790
scripts/flows/up.sh /tmp/flows --reset    # rebuilds the fixture after a flow changed it
scripts/flows/down.sh /tmp/flows          # stops the forge; the fixture stays
```

`up.sh` prints the environment to start Loupe with. Always set both `LOUPE_STRESS` and `HOME`: in scenario mode
Loupe uses its own throwaway settings domain and an in-memory account, and never reads or writes your real
settings or Keychain.

```sh
LOUPE_STRESS=/tmp/flows/scenario.json HOME=/tmp/flows/home GIT_CONFIG_NOSYSTEM=1 target/debug/loupe
```

| File | What it does |
|---|---|
| `mkflows.py` | Builds the repositories, worktrees, history and `scenario.json`. `FLOWS_PORT` changes the port. |
| `merge-fixture.sh` | Adds `repos/extra`: merge, rebase, cherry-pick, revert and special (symlink, submodule, binary) conflicts. |
| `mockforge-flows.py` | The mock Forgejo: API, git over HTTP, CI logs. Token `flows-token`, user `alice`. |

Test controls on the mock: `POST /_mock/fault` (make the next requests fail), `POST /_mock/version`,
`POST /_mock/pr` (edit a pull request). A stub `claude` in `home/.local/bin` answers commit-message and
conflict prompts; `LOUPE_FLOWS_AI` points Loupe at it.
