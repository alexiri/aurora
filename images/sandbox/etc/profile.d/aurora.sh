# Aurora sandbox image (FIX-138): the one PATH, and the interactive defaults of the web terminal.
# Read by bash login shells through /etc/profile: the web terminal and SSH `shell` both start one.
# Keep PATH identical to the image's ENV PATH, /etc/zshenv and the supervisor's spawn::IMAGE_PATH.
PATH=/workspace/.local/bin:/workspace/.local/share/gem/bin:/opt/aurora/bin:/usr/local/bin:/usr/bin:/bin
export PATH

# Everything below is for interactive shells only: agent turns (non-interactive) never see it.
case "$-" in
*i*) ;;
*) return 0 2>/dev/null || exit 0 ;;
esac
[ -n "${BASH_VERSION:-}" ] || return 0

export COLORTERM="${COLORTERM:-truecolor}"
if command -v dircolors >/dev/null 2>&1; then
    eval "$(dircolors -b)"
fi
alias ls='ls --color=auto'
alias ll='ls -l'
alias la='ls -A'
alias l='ls -CF'
alias grep='grep --color=auto'

HISTSIZE=10000
HISTFILESIZE=20000
HISTCONTROL=ignoreboth:erasedups
shopt -s histappend checkwinsize

# user@host:cwd — the sandbox's hostname carries the agent's handle (`sbx-<handle>-<boot>`).
PS1='\[\e[1;32m\]\u@\h\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ '

# (FIX-218, D-459(d)) A login bash reads ~/.bash_profile, else ~/.bash_login, else ~/.profile —
# never ~/.bashrc. Both entry points (the web terminal and SSH) start a login shell, so when the
# person's home has none of those files, read their own ~/.bashrc here: it is then honoured the
# way it is for an interactive non-login bash. A person with a profile file keeps full control
# (their profile can source ~/.bashrc itself). The unexported flag guards a double sourcing
# (a ~/.bashrc that sources /etc/profile again), and the shell variable never reaches children.
if shopt -q login_shell 2>/dev/null && [ -z "${__aurora_bashrc_read:-}" ] &&
    [ ! -e "$HOME/.bash_profile" ] && [ ! -e "$HOME/.bash_login" ] && [ ! -e "$HOME/.profile" ] &&
    [ -f "$HOME/.bashrc" ]; then
    __aurora_bashrc_read=1
    . "$HOME/.bashrc"
fi
