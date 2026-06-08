local micro = import("micro")

-- postinit() 会在 micro 主程序以及所有插件加载完毕后，最后自动运行一次
function postinit()
    -- 启动时，自动开启右侧的代码缩略图 (Minimap)
    -- 由于 filemanager.openonstart=true 已经在 settings.json 中开启，
    -- 这将自动形成 [左侧目录树 + 中间编辑区 + 右侧缩略图] 的三栏布局
    micro.CurPane():Command("OmniMinimap")
end
