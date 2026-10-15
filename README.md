# Voxferro

Editor de voxels en Rust, nativo para Apple Silicon, compatible con el formato
`.vox` de the reference editor.

Proyecto de aprendizaje: el objetivo es entender gráficos modernos y Rust
construyendo algo real, no clonar the reference editor línea por línea. La
interoperabilidad de archivos sí es un objetivo; la arquitectura es propia e
idiomática de Rust.

## Estado

Las fases 0–13 del [ROADMAP](ROADMAP.md) están completas, con **92 tests
verdes**; las 14–19 (editor) están planificadas. Funciona hoy:

- Lectura **y escritura** de `.vox` (versiones 150 y 200), validadas contra el
  corpus real y contra `dot_vox`
- Visor con mallado por oclusión de caras, paleta y cámara orbital
- Grafo de escena con transformaciones y frames de animación
- Edición con el ratón, con undo/redo que colapsa cada trazo en un paso
- Paleta de 256 colores, capas y consola de comandos
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
```

En la consola del panel izquierdo: `help`, `fill <c>`, `box …`, `sphere <c>`,
`gpu sphere|torus|box|shell`, `env uniform|atmosphere [elev]|ibl <mapa>`, `save [ruta]`, `undo`, `redo`. También Cmd+S.

## Stack

| Pieza | Elección | Por qué |
|---|---|---|
| Ventana + UI | `eframe` / `egui` 0.36 | Trae wgpu por defecto y fija una `winit` estable |
| Gráficos | `wgpu` 30 | Emite **Metal nativo** en Apple Silicon |
| Matemáticas | `glam` 0.34 | SIMD, estándar del ecosistema |
| GPU bytes | `bytemuck` 1 | `#[derive(Pod)]` para los buffers |

`eframe` y `wgpu` resuelven a una **única** `wgpu v30.0.1` en el grafo. Si
aparecen dos versiones, los tipos dejan de ser asignables entre sí y los
errores se vuelven incomprensibles: verificar con `cargo tree | grep wgpu`
antes de tocar esas versiones.

## Estructura

```
crates/
├── vox-format/   parser .vox          — sin dependencias gráficas
├── voxel-core/   volumen, grafo, undo — sin dependencias gráficas
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
