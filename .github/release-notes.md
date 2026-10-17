A voxel editor in Rust, compatible with the reference editor's `.vox` files. See the
[README](https://github.com/0xshellcode/voxferro#readme) for what it does.

## Downloads

| System | File | How to run |
|---|---|---|
| macOS (Apple silicon) | `Voxferro-*-macos-arm64.dmg` (or `.zip`) | Open the disk image and drag Voxferro to Applications. It is not signed: the first time, right-click it in Applications and choose **Open**, then **Open** again. |
| Windows (x64) | `Voxferro-*-windows-x64.zip` | Unzip anywhere and run `voxferro.exe`. Render mode uses DirectX 12 with Microsoft's DXC (`dxcompiler.dll`, `dxil.dll`, under `LICENSE-MS.txt`) included beside it. |
| Linux (x86-64) | `voxferro-*-linux-x86_64.tar.gz` | `tar -xzf voxferro-*.tar.gz -C ~/.local`, then run `voxferro` or find it in the applications menu. Needs Vulkan (Mesa or the GPU's driver). |

the reference editor's sample models, palettes and environment maps are not
included (they are its own); if the reference editor is installed in
`~/Applications/the reference editor`, Voxferro finds them there.
