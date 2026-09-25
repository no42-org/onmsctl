---
title: Shell completions
description: Generate and install onmsctl shell completion scripts.
---

```sh
onmsctl completion bash > /etc/bash_completion.d/onmsctl
onmsctl completion fish > ~/.config/fish/completions/onmsctl.fish
onmsctl completion zsh  > "$(brew --prefix)/share/zsh/site-functions/_onmsctl"   # adjust target to your setup
```

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

For Oh My Zsh, write to `~/.oh-my-zsh/custom/completions/_onmsctl` and add
`fpath=("$ZSH_CUSTOM/completions" $fpath)` to `~/.zshrc` above the
`source $ZSH/oh-my-zsh.sh` line.
The script targets the literal name `onmsctl`.
Run `sed -e 's/onmsctl/<name>/g'` if you've repackaged it.
