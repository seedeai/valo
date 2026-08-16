# valo-web

WebGPU-native 2D rendering through Valo's raw retained API or a typed,
Canvas-shaped adapter.

```ts
import { createValoCanvas } from "valo-web";

const context = await createValoCanvas(document.querySelector("canvas")!);
context.fillStyle = "#c8ff3d";
context.roundRect(24, 24, 240, 140, [28, 8, 28, 8]);
context.fill();
```

Import `valo-web/raw` for `DisplayListBuilder`, paths, paint, shaders,
paragraphs, image upload, render statistics, and explicit resource lifetimes.

The adapter supports fixed-font `fillText`, `strokeText`, and Canvas-shaped
`measureText` metrics. It intentionally throws `NotSupportedError` for
synchronous pixel readback/writes, `isPointInStroke`, and non-repeating pattern
modes. Register web fonts from bytes with `context.registerFont`; Valo never
silently uses browser fallback. Canvas `filter` supports ordered `blur`,
`brightness`, `contrast`, `grayscale`, `hue-rotate`, `invert`, `opacity`,
`saturate`, and `sepia` chains. `drop-shadow()` and `url()` remain explicit
gaps; invalid assignments are ignored like Canvas2D. `setColorMatrix` remains
available for a direct 4×5 matrix.

See the [Valo repository](https://github.com/tyxu/valo) for the playground,
Rust API, examples, and complete documentation.
