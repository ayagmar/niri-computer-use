// A page's name as man prints it in a running head: the page's last path part in capitals,
// or its group's for a group's overview, and a section, 1 for commands, 5 for file formats
// and 7 for the rest.

const SECTIONS: Record<string, number> = { 'reference/cli': 1, 'concepts/configuration': 5 };

export interface ManName {
	name: string;
	section: number;
}

export function manName(id: string): ManName {
	const parts = id.split('/');
	const last = parts[parts.length - 1];
	const word = last === 'overview' && parts.length > 1 ? parts[parts.length - 2] : last;
	return { name: word.toUpperCase(), section: SECTIONS[id] ?? 7 };
}

/** As a running head prints it, such as `SAFETY(7)`. */
export function heading({ name, section }: ManName): string {
	return `${name}(${section})`;
}

/** As SEE ALSO lists it, such as `safety(7)`. */
export function reference({ name, section }: ManName): string {
	return `${name.toLowerCase()}(${section})`;
}
