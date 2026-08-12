import type { CanvasCommand, CanvasScene } from "./scene.js";

export function makeBenchmarkScene(): CanvasScene {
  const commands: CanvasCommand[] = [];
  const colors = ["#ff4f79", "#d2ff45", "#6954ff", "#35a7ff", "#ff9f43"];
  for (let row = 0; row < 8; row += 1) {
    for (let column = 0; column < 8; column += 1) {
      const x = column * 16 + 2;
      const y = row * 16 + 2;
      const color = colors[(row * 3 + column * 2) % colors.length]!;
      commands.push(
        { type: "save" },
        { type: "translate", x: x + 7, y: y + 7 },
        { type: "rotate", radians: (row - column) * 0.025 },
        { type: "setFillColor", color },
        { type: "beginPath" },
        { type: "roundRect", x: -7, y: -7, width: 14, height: 14, radii: [3, 1, 3, 1] },
        { type: "fill", rule: "nonzero" },
        { type: "restore" },
      );
    }
  }
  return {
    name: "submission-benchmark",
    background: "#101218",
    commands,
  };
}
