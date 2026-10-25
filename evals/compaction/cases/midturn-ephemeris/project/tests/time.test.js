import test from 'node:test';
import assert from 'node:assert/strict';
import { julianMillennia } from '../src/astro/time.js';
test('J2000 is zero', () => assert.equal(julianMillennia(2451545.0), 0));
