# Voxferro

Editor de voxels en Rust, nativo para Apple Silicon, compatible con el formato
`.vox` de the reference editor.

Proyecto de aprendizaje: el objetivo es entender gráficos modernos y Rust
construyendo algo real, no clonar the reference editor línea por línea. La
interoperabilidad de archivos sí es un objetivo; la arquitectura es propia e
idiomática de Rust.

## Estado

Fase 0 completada: workspace, dependencias verificadas y andamiaje.
Ver [ROADMAP.md](ROADMAP.md).

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

## Uso

```bash
cargo test              # tests del parser y del core (rápido)
cargo run -p voxferro        # abre la ventana
```

## Licencia y alcance

Implementación independiente. El formato `.vox` está documentado públicamente
por su autor, its author. Este repositorio no contiene código ni assets de
the reference editor; los modelos de prueba se mantienen fuera del control de versiones
(ver `assets/README.md`).
