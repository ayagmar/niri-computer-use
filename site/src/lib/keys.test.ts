import { test } from 'node:test';
import assert from 'node:assert/strict';
import { keySwitch } from './keys.ts';

const refused = () => {
	throw new DOMException('denied', 'SecurityError');
};

test('turning the keys off works when the browser refuses storage', () => {
	const keys = keySwitch(refused);
	assert.equal(keys.on(), true);
	keys.set(false);
	assert.equal(keys.on(), false);
});

test('the choice is kept for the next page when storage works', () => {
	const saved = new Map<string, string>();
	const store = () => ({ getItem: (k: string) => saved.get(k) ?? null, setItem: (k: string, v: string) => void saved.set(k, v) });
	keySwitch(store).set(false);
	assert.equal(keySwitch(store).on(), false);
});
