# Crux (`crux`)

**From a game idea to a validated, playable prototype.**

Crux is an AI-agent game studio in a local dashboard. Pitch a one-line idea and a
crew of agents takes it down the funnel: an **idea-foundry** captures it into a
thesis, an **analyst** scores it against a 35-signal market-fit rubric, a
**producer** greenlights the strong ones, and a **game-designer** builds an actual
playable — vanilla Three.js, mobile-first, loop-first. You approve each hand-off.

> Crux is a fork of [alphaXiv/openresearch-cli](https://github.com/alphaXiv/openresearch-cli).
> The `crux` command is the CLI (`orx` still works as an alias). The
> experiment/run engine is shared — Crux is the game-discovery layer on top, and
> the general research agent is still available as a fallback persona.

## Quick start

Build from source — requires Rust (stable) via [rustup](https://rustup.rs). The
prebuilt dashboard UI is committed at `ui/dist`, so a plain build works:

```sh
git clone https://github.com/polats/crux
cd crux
cargo build --release           # binary at target/release/crux
./target/release/crux up
```

The dashboard opens at `http://127.0.0.1:3333`. Pick a model — Claude Code, Codex,
or OpenCode, whichever you have installed — then pitch a game:

```
a cozy game about brewing tea
```

The Producer captures it into a thesis, gets it scored, and leads you toward a
playable, suggesting the next agent for you to approve as it goes.

## The dashboard

`crux up` runs a single local process on `127.0.0.1` — an embedded web UI plus a
JSON/SSE API over a local SQLite store. Everything binds to loopback; your code and
data stay on your machine except the compute you initiate.

- **Agent chat** — talk to the Producer (or any persona), backed by your locally
  installed harness: Claude Code, Codex, or OpenCode. Pick the persona from the
  badge at the top of the chat; the pipeline routes you to the others.
- **The gallery** — every idea and prototype is a git branch. Roots are **ideas**;
  children are built **prototypes**, each with a play build, evaluation, code, and
  logs.
- **Play builds** — a prototype builds to a self-contained web game, playable right
  in the dashboard at `/play/<id>/`.
- **Evaluations** — the analyst scores an idea against the 35-signal rubric and
  files the read on the node, on the branch, and in Files.
- **Appearance** — retint the whole workspace with themes drawn from Sanzo Wada's
  *A Dictionary of Color Combinations*.

## How the pipeline works

1. **Capture** — the *idea-foundry* interviews a vague seed into a thesis with a
   real organic-pull hook and a retention scaffold.
2. **Evaluate** — the *analyst* codes 35 market-fit signals, runs a deterministic
   scoring sim, and writes a scored read (archetype, strengths, risks).
3. **Greenlight** — the *producer* orchestrates the funnel, suggesting the next
   agent for you to approve. Weak ideas iterate; only strong, greenlit ones build.
4. **Build** — the *game-designer* scaffolds from the committed UI starter (Three.js
   + Vite, a toon/juice substrate) and builds the **playable core loop first** —
   verified by a headless playability gate — then a thin meta layer.

Each agent runs in its own git worktree. Suggestions arrive as approval cards, and
the spawned agent nests under the one that suggested it, so the funnel stays legible.

## Compute

Prototypes build on this machine by default. For heavier runs, point them at Modal,
SSH boxes, Kubernetes, or Slurm — set it up once in **Settings → Compute** and
agents pick the right target per run.

### On a remote machine

Develop from your laptop while the dashboard runs next to your GPUs:

```sh
crux up --remote user@host      # or an ~/.ssh/config alias; append :PORT for a custom SSH port
```

This starts `crux up` on the remote box over SSH, tunnels the port back, and opens
your browser locally. The remote server is unauthenticated on that host's loopback.

## Commands

Run `crux --help` (or `crux <command> --help`) for full usage. The highlights:

| Area | Commands |
|---|---|
| Dashboard | `up` |
| Projects | `projects`, `project`, `create-project`, `env` |
| Ideas & prototypes | `experiments`, `create-experiment`, `exp status/run/cancel` |
| Runs & evidence | `runs`, `logs`, `search-logs`, `artifacts`, `query` |
| Agents | `agent suggest/list/status`, `install-skills`, `skill` |
| Compute | `compute`, `instance create` |
| Maintenance | `version`, `update`, `telemetry` |

`crux install-skills` drops the Crux skill into your local coding agents (Claude
Code, Codex, OpenCode, Cursor) so they can drive `crux` themselves.

## Usage analytics

`crux` sends **anonymous**, opt-out usage analytics — command name, a random
per-install UUID, version, OS/arch, and coarse event labels. Never collected: code,
prompts, file contents or paths, project/experiment ids or names, tokens, or emails.

```sh
crux telemetry off        # persistent, per-machine
crux telemetry status     # current state + the anonymous install id
crux <cmd> --no-telemetry # per-run
```

## License

MIT.
