# debopti 覆盖：puffer-fish 上游在 Fish 4.0+ 使用 commandline --search-field，
# Debian 10~12 的 apt fish 3.x 不支持该选项，输入 '.' 会报错。
function _puffer_fish_expand_dot
    set -l fish_major (string split '.' -- $FISH_VERSION)[1]
    if test "$fish_major" -ge 4 2>/dev/null
        if commandline --search-field >/dev/null 2>&1
            commandline --search-field --insert '.'
            return
        end
    end
    if string match --quiet --regex -- '^(\.\./)*\.\.$' (commandline --current-token)
        commandline --insert '/..'
    else
        commandline --insert '.'
    end
end
