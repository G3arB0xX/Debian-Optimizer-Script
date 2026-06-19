# MicroOmni upstream ff28759e: getSessionFilePath 误用 Lua os 而非 import("os") 的 goos
s/os\.MkdirAll(sessionsPath, os\.ModePerm)/goos.MkdirAll(sessionsPath, goos.ModePerm)/
