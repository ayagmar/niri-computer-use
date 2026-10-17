// Whether less's keys are on. The state lives in memory, so turning them off works even
// where the browser refuses storage; storage only carries the choice across pages.
type Store = Pick<Storage, 'getItem' | 'setItem'>;

export function keySwitch(storage: () => Store) {
	let on = true;
	try {
		on = storage().getItem('ncuKeys') != 'off';
	} catch {
		// No storage: the keys start on.
	}
	return {
		on: () => on,
		set(value: boolean) {
			on = value;
			try {
				storage().setItem('ncuKeys', value ? 'on' : 'off');
			} catch {
				// No storage: the choice lasts until the page is left.
			}
		},
	};
}
