---
title: Shell completions
description: Generate and install onmsctl shell completion scripts.
---

`onmsctl completion <shell>` prints a completion script to stdout.
Supported shells: `bash`, `elvish`, `fish`, `powershell`, `zsh`.
The command runs locally and needs no server.

## Install the completion script

| Shell | Command | Target path |
|---|---|---|
| bash (user) | `mkdir -p ~/.local/share/bash-completion/completions && onmsctl completion bash > ~/.local/share/bash-completion/completions/onmsctl` | `~/.local/share/bash-completion/completions/onmsctl` |
| bash (system) | `onmsctl completion bash \| sudo tee /etc/bash_completion.d/onmsctl > /dev/null` | `/etc/bash_completion.d/onmsctl` |
| zsh (Homebrew) | `onmsctl completion zsh > "$(brew --prefix)/share/zsh/site-functions/_onmsctl"` | `$(brew --prefix)/share/zsh/site-functions/_onmsctl` |
| zsh (no file) | `eval "$(onmsctl completion zsh)"` in `~/.zshrc`, after `compinit` | none |
| fish | `onmsctl completion fish > ~/.config/fish/completions/onmsctl.fish` | `~/.config/fish/completions/onmsctl.fish` |
| elvish | `eval (onmsctl completion elvish \| slurp)` in `~/.config/elvish/rc.elv` | none |
| powershell | `onmsctl completion powershell \| Out-String \| Invoke-Expression` in your `$PROFILE` | none |

Adjust the target path to your setup.
The elvish and powershell rows follow each shell's standard loading pattern.
Open a new shell after installing a script file.

### Load zsh completions with Oh My Zsh

This follows Oh My Zsh's own convention, not anything onmsctl controls.
Write the script to `~/.oh-my-zsh/custom/completions/_onmsctl`.
Add `fpath=("$ZSH_CUSTOM/completions" $fpath)` to `~/.zshrc` above the `source $ZSH/oh-my-zsh.sh` line.

## Rename the completed command

The script targets the literal name **`onmsctl`**.
If you repackaged the binary under another name, rewrite the script with `sed`.
This example installs bash completions for a binary named `mynms`:

```sh
onmsctl completion bash | sed -e 's/onmsctl/mynms/g' > ~/.local/share/bash-completion/completions/mynms
```
