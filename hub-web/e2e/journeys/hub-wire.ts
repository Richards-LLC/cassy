import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import schemas from "../../test/fixtures/hub-wire.schema.json" with { type: "json" };

export const hubWireValidator = addFormats(new Ajv2020({
  strict: false,
  schemas: Object.values(schemas),
  formats: { uint8: true, uint16: true, uint32: true, uint64: true },
}));

export function assertHubWire(schema: keyof typeof schemas, payload: unknown): void {
  const id = `urn:cassy:hub:${schema}`;
  const validate = hubWireValidator.getSchema(id);
  if (!validate) throw new Error(`Unknown hub wire schema: ${id}`);
  if (!validate(payload)) throw new Error(`${id}: ${hubWireValidator.errorsText(validate.errors)}`);
}
