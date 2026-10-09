# Agents at the controls (MCP)

`voxl-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server for a running
game. With it an AI agent can do what a developer at the game can: look at the scene, read
and change any registered state, stop and step time, go back, read failures with their
stacks, watch and rewire the game's rules, and reload plugins.

## Setting it up

Start a game that listens for debuggers, and tell the agent's host about the server:

```sh
VOXL_DEBUG=127.0.0.1:7878 cargo run --example host -- chase
claude mcp add voxl -- cargo run --quiet --manifest-path /path/to/voxl/Cargo.toml --bin voxl-mcp
```

Any MCP client works the same way: it runs `voxl-mcp` and speaks to it over standard input
and output. `voxl-mcp` finds the game at `--at address`, else at `VOXL_DEBUG`, else at
127.0.0.1:7878. The game need not be running when the server starts; each tool call connects
afresh and says so plainly if nothing answers. `examples/sacred_sites` is a game with no
window to try the tools on.

## The tools

Each tool is one command of the [debug connection](LIVE.md#the-debug-connection), named
`voxl_` and the command.

| Tools | For |
| --- | --- |
| `voxl_status` | where the game is: frame, time, paused, failures, entity count |
| `voxl_screenshot` | **seeing the scene**: the next rendered frame as an image, scaled to `width` (1024) |
| `voxl_entities`, `voxl_get`, `voxl_set`, `voxl_remove`, `voxl_spawn`, `voxl_despawn` | entities and their components, by name |
| `voxl_resource` | game-wide settings, read or set |
| `voxl_types`, `voxl_schema` | what can be read and written, and the shape of each type |
| `voxl_systems`, `voxl_failures` | what runs, what it touches, how long it takes, what broke and where |
| `voxl_pause`, `voxl_resume`, `voxl_step`, `voxl_time_scale` | time |
| `voxl_record`, `voxl_history`, `voxl_rewind` | going back |
| `voxl_signals`, `voxl_signal_set`, `voxl_signal_force`, `voxl_signal_connect`, `voxl_signal_define`, `voxl_signal_remove` | the [signal graph](SIGNALS.md): the game's rules, drawn and listed, and changed live |
| `voxl_reload_plugins`, `voxl_save_scene` | code and data |

A command the game refuses comes back as a tool result marked as an error, with the reason in
words, so the agent can read it and try something else.

## What an agent can know

Everything registered with the [type registry](SCENES.md): `voxl_types` lists it and
`voxl_schema` describes it. State that is not registered is invisible here, as it is to
scenes and to stepping back; that is the reason to register a game's components and
resources. A plugin's components appear once the plugin
[describes](PLUGINS.md#describing-components) them.

## What isn't here yet

- The screenshot is the game's own view. A chosen camera or free viewpoint, and overlays
  (entity ids, colliders, the signal graph), are to come; so is a description of the scene in
  words.
- Nothing is pushed: an agent finds out about a failure or a signal changing by asking.
- The server doesn't launch games, run them without a window, or step them deterministically
  to a condition; `voxl_step` returns at once and the frames run as the game's loop comes
  round.
- No input from the agent (keys, mouse), so it can change the game but not yet play it.
- Anyone who can reach the debug address can do all of this. Keep it on the machine.
