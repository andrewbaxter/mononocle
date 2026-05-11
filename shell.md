# Shell integration: auto set-desktop

Two mechanisms are provided. Use either or both.

## 1. On terminal focus (requires terminal support)

Most modern terminals (kitty, foot, alacritty, wezterm, konsole, etc.) support
`\e[?1004h` focus reporting. When enabled, the terminal sends `\e[I` on
focus-in and `\e[O` on focus-out. We bind focus-in to `mononocli set-desktop`
so the terminal's PID tree is associated with the current desktop whenever you
switch to it.

### zsh

```zsh
# -- mononocle: associate terminal with desktop on focus --
printf '\e[?1004h'                        # enable focus reporting

_mn_focus_in() { mononocli set-desktop &>/dev/null &! }
_mn_focus_out() { }                       # consume silently

zle -N _mn_focus_in
zle -N _mn_focus_out
bindkey '\e[I' _mn_focus_in
bindkey '\e[O' _mn_focus_out

_mn_focus_cleanup() { printf '\e[?1004l' }
add-zsh-hook zshexit _mn_focus_cleanup
```

Requires `autoload -Uz add-zsh-hook` earlier in your zshrc (most setups
already have this).

### bash

```bash
# -- mononocle: associate terminal with desktop on focus --
printf '\e[?1004h'                        # enable focus reporting
trap 'printf "\e[?1004l"' EXIT            # disable on exit

bind -x '"\e[I": mononocli set-desktop &>/dev/null'
bind    '"\e[O": ""'                      # consume focus-out silently
```

`bind -x` lets readline run a shell command when a key sequence arrives.

### Caveats

- Only fires when readline/zle is active (i.e. you're at a prompt). Focus
  changes during a running command are ignored, which is fine — the command's
  own wayland windows will already be tracked by PID ancestry.
- If your terminal doesn't support `\e[?1004h` the sequences are silently
  ignored and nothing breaks.
- `\e[O` must be consumed or it'll print garbage at the prompt. The binding
  above handles that. `\e[O` is also the SS3 prefix for some key sequences
  (F1-F4, arrow keys on some terminals), but focus-out is `\e[O` as a
  complete sequence (no trailing byte), so it doesn't conflict with `\e[OA`
  etc. — readline/zle wait for more input on ambiguous prefixes.

## 2. Before and after each command

Runs `set-desktop` both right before a command executes (`preexec`) and right
after it finishes (`precmd`/`PROMPT_COMMAND`). This ensures the PID tree stays
associated even if you switch desktops while a long-running command is active —
the association is refreshed as soon as you return to the prompt.

### zsh

```zsh
# -- mononocle: associate terminal with desktop on every prompt/exec --
_mn_set_desktop() { mononocli set-desktop &>/dev/null &! }

add-zsh-hook precmd  _mn_set_desktop      # after command finishes / prompt draw
add-zsh-hook preexec _mn_set_desktop      # just before command runs
```

### bash

```bash
# -- mononocle: associate terminal with desktop on every prompt/exec --
_mn_set_desktop() { mononocli set-desktop &>/dev/null; }

# preexec via DEBUG trap (fires before each command)
trap '_mn_set_desktop' DEBUG

# precmd via PROMPT_COMMAND (fires before each prompt)
PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}_mn_set_desktop"
```

### Caveats

- Each hook spawns a short-lived IPC round-trip. The latency is negligible
  (Unix socket + no compositor work beyond a HashMap insert), but if you
  notice it, drop the `preexec`/`DEBUG` hook and keep only the
  `precmd`/`PROMPT_COMMAND` one.
- In bash, `trap ... DEBUG` fires for every simple command in a pipeline. The
  overhead is still minimal, but if you run complex compound commands you can
  guard it: `trap '[[ "$BASH_COMMAND" != _mn_* ]] && _mn_set_desktop' DEBUG`.
