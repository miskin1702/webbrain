import { normalizeTitle } from './title.js';

export function renderHeader(rawTitle) {
  const clean = normalizeTitle(rawTitle);
  return `<h1>${clean}</h1>`;
}
