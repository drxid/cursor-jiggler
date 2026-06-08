import { Resvg } from "@resvg/resvg-js";
import { readFileSync, writeFileSync } from "node:fs";

const svg = readFileSync("reference/app-icon-white.svg", "utf8");

function render(size, out) {
  const r = new Resvg(svg, { fitTo: { mode: "width", value: size } });
  writeFileSync(out, r.render().asPng());
  console.log("wrote", out, `${size}px`);
}

render(1024, "src-tauri/app-icon.png");
render(128, "src-tauri/icons/tray-normal.png");
