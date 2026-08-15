local micro = import("micro")
local config = import("micro/config")
local util = import("micro/util")
local shell = import("micro/shell")

local function line_at(bp, y)
    if bp.Buf.LineBytes ~= nil then
        local ok, bytes = pcall(function() return bp.Buf:LineBytes(y) end)
        if ok and bytes ~= nil then
            return util.String(bytes)
        end
    end
    if bp.Buf.Line ~= nil then
        local ok, s = pcall(function() return bp.Buf:Line(y) end)
        if ok and type(s) == "string" then
            return s
        end
    end
    return ""
end

local function cursor_y(c)
    if c.Loc ~= nil and c.Loc.Y ~= nil then
        return c.Loc.Y
    end
    if c.Y ~= nil then
        return c.Y
    end
    return 0
end

local function text_for_copy(bp)
    local c = bp.Cursor
    if c:HasSelection() then
        return util.String(c:GetSelection())
    end
    local copyline = true
    pcall(function()
        copyline = config.GetGlobalOption("copyline")
    end)
    if copyline == false or copyline == "false" then
        return ""
    end
    return line_at(bp, cursor_y(c)) .. "\n"
end

local function send_host_clipboard(text)
    if text == nil or text == "" then
        return
    end
    local tmp = os.tmpname()
    local f = io.open(tmp, "wb")
    if f == nil then
        return
    end
    f:write(text)
    f:close()
    local ok = pcall(function()
        shell.ExecCommand("debopti-osc52", "-i", "clipboard", tmp)
    end)
    if not ok then
        pcall(function()
            shell.RunCommand("debopti-osc52 -i clipboard " .. tmp)
        end)
    end
    os.remove(tmp)
end

-- 仅把当前选区/行发到主机；内部剪贴板由绑定链里的原生 Copy/Cut 负责
function deboptiOsc52(bp)
    local text = ""
    pcall(function() text = text_for_copy(bp) end)
    send_host_clipboard(text)
end

function deboptiCut(bp)
    local text = ""
    pcall(function() text = text_for_copy(bp) end)
    pcall(function() bp:Cut() end)
    send_host_clipboard(text)
end

function init()
    config.MakeCommand("deboptiOsc52", deboptiOsc52, config.NoComplete)
    config.MakeCommand("deboptiCut", deboptiCut, config.NoComplete)
    config.MakeCommand("deboptiCopy", deboptiOsc52, config.NoComplete)
    config.TryBindKey("Ctrl-c", "Copy,command:deboptiOsc52", true)
    config.TryBindKey("Ctrl-x", "command:deboptiCut", true)
end

function postinit()
    -- 默认呈现纯净的单栏编辑器窗口
end
