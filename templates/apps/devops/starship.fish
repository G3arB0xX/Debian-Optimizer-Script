if command -v starship >/dev/null 2>&1
    starship init fish | source
else if test -f /usr/local/bin/starship
    /usr/local/bin/starship init fish | source
end
