/** The same shadowed letters the terminal prints. */
const GLYPHS: Record<string, readonly string[]> = {
  K: ["██╗  ██╗", "██║ ██╔╝", "█████╔╝ ", "██╔═██╗ ", "██║  ██╗", "╚═╝  ╚═╝"],
  I: ["██╗", "██║", "██║", "██║", "██║", "╚═╝"],
  N: ["███╗   ██╗", "████╗  ██║", "██╔██╗ ██║", "██║╚██╗██║", "██║ ╚████║", "╚═╝  ╚═══╝"],
  E: ["███████╗", "██╔════╝", "█████╗  ", "██╔══╝  ", "███████╗", "╚══════╝"],
  T: ["████████╗ ", "╚══██╔══╝ ", "   ██║    ", "   ██║    ", "   ██║    ", "   ╚═╝    "],
  C: [" ██████╗", "██╔════╝", "██║     ", "██║     ", "╚██████╗", " ╚═════╝"],
  V: ["██╗   ██╗", "██║   ██║", "██║   ██║", "╚██╗ ██╔╝", " ╚████╔╝ ", "  ╚═══╝  "],
  M: ["███╗   ███╗", "████╗ ████║", "██╔████╔██║", "██║╚██╔╝██║", "██║ ╚═╝ ██║", "╚═╝     ╚═╝"],
};

function compose(word: string): string[] {
  const rows = ["", "", "", "", "", ""];
  for (const ch of word) {
    const glyph = GLYPHS[ch] ?? [" ", " ", " ", " ", " ", " "];
    glyph.forEach((row, index) => {
      rows[index] += row;
    });
  }
  return rows;
}

const kinetic = compose("KINETIC");
const vm = compose("VM");

/** Full wordmark. KINETIC is purple; VM starts after this column. */
export const VM_COLUMN = kinetic[0].length + 3;

export const MARK_LINES = kinetic.map((row, index) => `${row}   ${vm[index]}`);

export const MARK_WIDTH = MARK_LINES[0].length;
