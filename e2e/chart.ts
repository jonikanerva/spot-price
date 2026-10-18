import type { Page } from "@playwright/test";

// The horizontal padding of drawChart in crates/server/src/http/ui/client.js.
export const PADDING_LEFT = 44;
const PADDING_RIGHT = 10;

export interface ChartGeometry {
  chartWidth: number;
  ticks: { label: string; x: number }[];
  paths: [number, number][][];
}

const parsePath = (d: string): [number, number][] =>
  d
    .split(/[ML]/)
    .map((command) => command.trim())
    .filter((command) => command.length > 0)
    .map((command) => {
      const [x = Number.NaN, y = Number.NaN] = command.split(" ").map(Number);
      return [x, y];
    });

/** The x-axis labels and the line coordinates of a drawn chart. */
export const readChart = async (
  page: Page,
  svgId: string,
): Promise<ChartGeometry> => {
  const raw = await page.locator(`#${svgId}`).evaluate((svg) => ({
    width: (svg as SVGSVGElement).viewBox.baseVal.width,
    ticks: Array.from(svg.querySelectorAll('text[text-anchor="middle"]')).map(
      (text) => ({
        label: text.textContent ?? "",
        x: Number(text.getAttribute("x")),
      }),
    ),
    paths: Array.from(svg.querySelectorAll("path")).map(
      (path) => path.getAttribute("d") ?? "",
    ),
  }));
  return {
    chartWidth: raw.width - PADDING_LEFT - PADDING_RIGHT,
    ticks: raw.ticks,
    paths: raw.paths.map(parsePath),
  };
};
