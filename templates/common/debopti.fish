# Debian Optimizer Script Fish 增强
if status is-interactive
    abbr -a debopti '/usr/local/bin/debopti'
end
if not contains /usr/local/bin $PATH
    set -gx PATH $PATH /usr/local/bin
end
