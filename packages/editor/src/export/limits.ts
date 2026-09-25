export class ExportLimitError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "ExportLimitError";
	}
}
