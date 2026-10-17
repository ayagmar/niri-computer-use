// Run by `npm test`, which `npm run build` runs first. Node strips the types itself.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { heading, manName, reference } from './man.ts';

test('commands are section 1, the policy file section 5, everything else 7', () => {
	assert.equal(heading(manName('reference/cli')), 'CLI(1)');
	assert.equal(heading(manName('concepts/configuration')), 'CONFIGURATION(5)');
	assert.equal(heading(manName('concepts/safety')), 'SAFETY(7)');
});

test("a group's overview is named after the group, and SEE ALSO is lower case", () => {
	assert.equal(heading(manName('tools/overview')), 'TOOLS(7)');
	assert.equal(reference(manName('start/install-script')), 'install-script(7)');
});
