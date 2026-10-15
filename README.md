# Voxferro

Editor de voxels en Rust, nativo para Apple Silicon, compatible con el formato
`.vox` de the reference editor.

Proyecto de aprendizaje: el objetivo es entender gráficos modernos y Rust
construyendo algo real, no clonar the reference editor línea por línea. La
interoperabilidad de archivos sí es un objetivo; la arquitectura es propia e
idiomática de Rust.

## Estado

Las fases 0–19 del [ROADMAP](ROADMAP.md) están completas, con **134 tests
verdes**. Funciona hoy:

- Lectura **y escritura** de `.vox` (versiones 150 y 200), validadas contra el
  corpus real y contra `dot_vox`
- Visor con mallado voraz (caras coplanares fusionadas), paleta y cámara orbital
- Grafo de escena con transformaciones y frames de animación; edición y render
  de escenas con varios modelos, rotados incluidos
- Abrir, nuevo, guardar como, y aviso de cambios sin guardar
- Pinceles añadir / borrar / pintar con formas voxel, caja, línea, cara y
  relleno, con vista previa y undo/redo de un paso por trazo
- Selección (marco, cara, color), copiar/cortar/pegar, mover; rotar, espejo,
  recortar y cambiar tamaño del modelo
- Paleta y materiales editables desde la interfaz, con undo
- Exportación a OBJ + MTL + textura de paleta
- Capas y consola de comandos
- Operaciones de voxel como compute kernels en WGSL
- Guardado atómico que conserva materiales, cámaras y ajustes del renderer
- Path tracer progresivo (GI, sombras, ACES) con la iluminación del archivo
- Materiales metal, vidrio y emisivos, con las fórmulas de the reference editor
- Plano de suelo infinito con fusión en el horizonte
- Iluminación con mapas HDR, orientada y normalizada como en the reference editor
- Cielo atmosférico con dispersión Rayleigh y Mie, y sol que enrojece al atardecer

Nativo arm64 sobre Metal: `[voxferro] graphics backend: Metal on Apple M2`.

## Uso

```bash
cargo run -p voxferro                                  # abre monu1.vox
cargo run -p voxferro -- assets/vox/castle.vox         # abre otro modelo
cargo run -p voxferro -- --run "gpu torus 166 40 14 replace" --run "save out.vox"
cargo run -p voxferro -- assets/vox/monu1.vox --render-mode           # abre en modo Render
cargo run -p voxferro -- assets/vox/monu1.vox --render 200 --screenshot out.bmp
cargo run -p voxferro -- --run "new 16" --run "brush attach box" --drag 0.3 0.7 0.6 0.8 --screenshot out.bmp
```

**Ratón:** pulsar sobre el modelo edita con el pincel; pulsar en el vacío (o
arrastrar con el botón derecho) orbita; la rueda acerca.

**Teclado:** Cmd+N/O/S, Cmd+Shift+S, Cmd+Z / Cmd+Shift+Z; con selección,
Cmd+A, Esc, Supr, Cmd+C/X/V, flechas y Re Pág/Av Pág para moverla.

**Consola** (panel izquierdo; `help` para los comandos de modelo):

| Grupo | Comandos |
|---|---|
| Archivos | `open [ruta]`, `new [n \| x y z]`, `save [ruta]`, `export <ruta.obj>` |
| Pincel | `brush [attach\|erase\|paint\|select] [voxel\|box\|line\|face\|fill]`, `color <n>` |
| Selección | `select all\|none\|invert\|color [n]`, `delete`, `paint [c]`, `copy`, `cut`, `paste`, `move dx dy dz` |
| Modelo | `rotate x\|y\|z`, `mirror x\|y\|z`, `crop`, `resize x y z`, `fill`, `clear`, `box`, `sphere`, `gpu …` |
| Paleta | `palette <i> <r> <g> <b>`, `material <i> <diffuse\|metal\|glass\|emit> [clave=valor]…` |
| Render | `env uniform\|atmosphere [elev]\|ibl <mapa> [i] [rot]` |

Para pruebas sin ratón: `--click fx fy` y `--drag fx0 fy0 fx1 fy1`, en
fracciones del visor.

## Stack

| Pieza | Elección | Por qué |
|---|---|---|
| Ventana + UI | `eframe` / `egui` 0.36 | Trae wgpu por defecto y fija una `winit` estable |
| Gráficos | `wgpu` 30 | Emite **Metal nativo** en Apple Silicon |
| Matemáticas | `glam` 0.34 | SIMD, estándar del ecosistema |
| GPU bytes | `bytemuck` 1 | `#[derive(Pod)]` para los buffers |
| Diálogos | `rfd` 0.17 | Paneles nativos de abrir y guardar |

`eframe` y `wgpu` resuelven a una **única** `wgpu v30.0.1` en el grafo. Si
aparecen dos versiones, los tipos dejan de ser asignables entre sí y los
errores se vuelven incomprensibles: verificar con `cargo tree | grep wgpu`
antes de tocar esas versiones.

## Estructura

```
crates/
├── vox-format/   parser .vox          — sin dependencias gráficas
├── voxel-core/   volumen, grafo, undo, pinceles, selección, mallado, OBJ
│                 — sin dependencias gráficas
└── app/          eframe + egui + wgpu
```

Los dos primeros crates no dependen de wgpu a propósito: `cargo test -p
vox-format` corre en un segundo en lugar de recompilar el stack gráfico.

### Tests

```bash
cargo test -p vox-format -p voxel-core   # rápido: no compila wgpu
cargo test                               # todo
```

## Licencia y alcance

Implementación independiente. El formato `.vox` está documentado públicamente
por su autor, its author. Este repositorio no contiene código ni assets de
the reference editor; los modelos de prueba se mantienen fuera del control de versiones
(ver `assets/README.md`).
