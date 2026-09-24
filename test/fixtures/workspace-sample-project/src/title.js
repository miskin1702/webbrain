/**
 * Normalizes title strings by trimming whitespace and collapsing multiple spaces.
 */
export function normalizeTitle(title) {
  if (!title) {
    return 'Untitled Recovered';
  }
  return title.trim().replace(/\s+/g, ' ');
}

// External change from VS Code
