# Agents at the controls (MCP)

`mira-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server for a running
game. With it an AI agent can do what a developer at the game can: look at the scene, read
and change any registered state, stop and step time, go back, read failures with their
stacks, watch and rewire the game's rules, and reload plugins.

## Setting it up

Start a game that listens for debuggers, and tell the agent's host about the server:

```sh
MIRA_DEBUG=127.0.0.1:7878 cargo run --example host -- chase
claude mcp add mira -- cargo run --quiet --manifest-path /path/to/mira/Cargo.toml --bin mira-mcp
```

Any MCP client works the same way: it runs `mira-mcp` and speaks to it over standard input
and output. `mira-mcp` finds the game at `--at address`, else at `MIRA_DEBUG`, else at
127.0.0.1:7878. The game need not be running when the server starts; each tool call connects
afresh and says so plainly if nothing answers. `examples/sacred_sites` is a game with no
window to try the tools on.

## The tools

Each tool is one command of the [debug connection](LIVE.md#the-debug-connection), named
`mira_` and the command.

| Tools | For |
| --- | --- |
| `mira_status` | where the game is: frame, time, paused, failures, entity count |
| `mira_screenshot` | **seeing the scene**: the next rendered frame as an image, scaled to `width` (1024) |
| `mira_describe` | the scene in words: the camera, and what there is, things on screen first and nearest first, with where on the screen each appears |
| `mira_entities`, `mira_get`, `mira_set`, `mira_remove`, `mira_spawn`, `mira_despawn` | entities and their components, by name |
| `mira_resource` | game-wide settings, read or set |
| `mira_types`, `mira_schema` | what can be read and written, and the shape of each type |
| `mira_systems`, `mira_failures` | what runs, what it touches, how long it takes, what broke and where |
| `mira_pause`, `mira_resume`, `mira_step`, `mira_time_scale` | time; `mira_step` waits for its frames and answers with the status after them |
| `mira_run_until` | runs until a [signal](SIGNALS.md) is true, then pauses: getting the game to a moment worth looking at |
| `mira_input` | **playing the game**: keys, mouse buttons, mouse motion and position, as if at the keyboard |
| `mira_record`, `mira_history`, `mira_rewind` | going back |
| `mira_signals`, `mira_signal_set`, `mira_signal_force`, `mira_signal_connect`, `mira_signal_define`, `mira_signal_remove` | the [signal graph](SIGNALS.md): the game's rules, drawn and listed, and changed live |
| `mira_reload_plugins`, `mira_save_scene` | code and data |

A command the game refuses comes back as a tool result marked as an error, with the reason in
words, so the agent can read it and try something else.

## Playing to a moment

An agent that has changed something wants to see what happens, and what happens usually takes
input and time. `mira_input` plays keys and the mouse into the game (a tap, or a press held
until released); it arrives at the start of the next simulated frame, so input played into a
paused game is there, pressed that very frame, when the game is stepped. `mira_run_until`
then runs the simulation until a signal is true, or a number of frames have passed, and
pauses:

```text
mira_pause
mira_input      {"key": "KeyD", "action": "press"}
mira_run_until  {"signal": "player.at_door", "max_frames": 600}   → {"reached": true, "frame": 412, …}
mira_screenshot
```

Any condition worth waiting for can be made a signal first, with `mira_signal_define` or in
the game's code.

## What an agent can know

Everything registered with the [type registry](SCENES.md): `mira_types` lists it and
`mira_schema` describes it. State that is not registered is invisible here, as it is to
scenes and to stepping back; that is the reason to register a game's components and
resources. A plugin's components appear once the plugin
[describes](PLUGINS.md#describing-components) them.

## What isn't here yet

- The screenshot is the game's own view. A chosen camera or free viewpoint, and overlays
  (entity ids, colliders, the signal graph), are to come.
- `mira_describe` places things by their origin; it doesn't know their size, or what hides
  what.
- Nothing is pushed through MCP: an agent finds out about a failure or a signal changing by
  asking. (The debug connection itself can push; see [LIVE.md](LIVE.md).)
- The server doesn't launch games or run them without a window. Stepped frames are each
  `Live::step` long (1/60 s), so a stepped run is repeatable as far as the game itself is.
- Input is keys, mouse buttons and mouse movement; no gamepad, no text entry.
- Anyone who can reach the debug address can do all of this. Keep it on the machine.
