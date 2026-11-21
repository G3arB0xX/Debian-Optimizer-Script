# debopty Fish 增强
if status is-interactive
    abbr -a debopty '{{BIN_LINK}}'
end
if not contains /usr/local/bin $PATH
    set -gx PATH $PATH /usr/local/bin
end
