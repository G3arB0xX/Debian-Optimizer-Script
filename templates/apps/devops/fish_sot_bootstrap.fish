# debopti SOT bootstrap — 每用户本地文件，由脚本渲染；勿手动与 SOT 文件同名覆盖
# 将 SOT 的 functions/completions 加入 autoload 路径（只读）；用户自定义放本地 functions/ 与 conf.d.local/

set -gx _debopti_sot_fish '{{SOT_FISH}}'

if test -d "$_debopti_sot_fish/functions"
    contains "$_debopti_sot_fish/functions" $fish_function_path
        or set -a fish_function_path "$_debopti_sot_fish/functions"
end

if test -d "$_debopti_sot_fish/completions"
    contains "$_debopti_sot_fish/completions" $fish_complete_path
        or set -a fish_complete_path "$_debopti_sot_fish/completions"
end

if test -d "$__fish_config_dir/conf.d.local"
    for _debopti_f in $__fish_config_dir/conf.d.local/*.fish
        source $_debopti_f
    end
end
