-- 自定义列表行元信息：权限 + 所有者（Unix）
-- 在 yazi.toml 中设置 linemode = "perm_owner" 启用

function Linemode:perm_owner()
	if ya.target_family() ~= "unix" then
		return ""
	end

	local perm = self._file.cha:perm() or ""
	local user = ya.user_name and ya.user_name(self._file.cha.uid) or self._file.cha.uid
	local group = ya.group_name and ya.group_name(self._file.cha.gid) or self._file.cha.gid
	return string.format("%s %s:%s", perm, user, group)
end
