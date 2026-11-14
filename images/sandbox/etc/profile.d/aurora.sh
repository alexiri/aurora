# Aurora sandbox image (FIX-138): the one PATH, and the interactive defaults of the web terminal.
# Read by bash login shells (/etc/profile) and by interactive non-login bash (/etc/bashrc).
# Keep PATH identical to the image's ENV PATH, /etc/zshenv and the supervisor's spawn::IMAGE_PATH.
PATH=/workspace/.local/bin:/opt/aurora/bin:/usr/local/bin:/usr/bin:/bin
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
