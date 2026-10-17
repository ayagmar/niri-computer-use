// The length of an MP4, read from its movie header, so the page states the clip's real length
// when the recording is replaced.

export function mp4Seconds(file: Uint8Array): number {
	const view = new DataView(file.buffer, file.byteOffset, file.byteLength);
	const at = Buffer.from(file).indexOf('mvhd');
	if (at < 0) throw new Error('no mvhd box in the MP4');
	// Version 1 headers carry 64-bit times; version 0, 32-bit.
	const v1 = view.getUint8(at + 4) === 1;
	const timescale = view.getUint32(at + (v1 ? 24 : 16));
	const duration = v1 ? Number(view.getBigUint64(at + 28)) : view.getUint32(at + 20);
	return duration / timescale;
}
