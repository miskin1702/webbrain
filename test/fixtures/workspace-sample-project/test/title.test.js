import { strict as assert } from 'node:assert';
import { normalizeTitle } from '../src/title.js';

// Basic normalization
assert.equal(normalizeTitle('  Hello   World  '), 'Hello World');
assert.equal(normalizeTitle('Title'), 'Title');

// Currently asserts empty string for empty input
assert.equal(normalizeTitle(''), 'Untitled');

console.log('✓ All title tests passed');
