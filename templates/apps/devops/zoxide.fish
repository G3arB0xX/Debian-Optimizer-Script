if command -v zoxide >/dev/null 2>&1
    zoxide init fish | source
else if test -f /usr/local/bin/zoxide
    /usr/local/bin/zoxide init fish | source
end
