// Required-set conformance of the generated API types (src/api/schema.d.ts)
// with the OpenAPI document they are generated from (openapi/openapi.json).
// For every component schema with properties, the declaration has exactly the
// schema's properties as members, and exactly its `required` list (none when
// absent) as members without `?`. gen:api:check proves the committed
// declaration is what the generator emits; this proves that what it emits
// keeps the document's requiredness, which, for example, a published
// `default` on a property does not.
import ts from "typescript";
import { describe, expect, it } from "vitest";

import openapiDocument from "../../openapi/openapi.json";
import schemaSource from "./schema.d.ts?raw";

import type { components } from "@/api/schema";

// The member names of one object schema, each list sorted.
interface Members {
  // Every property name.
  names: string[];
  // The property names a value must carry.
  required: string[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function sortedNames(value: unknown, where: string): string[] {
  if (
    !Array.isArray(value) ||
    !value.every((item): item is string => typeof item === "string")
  ) {
    throw new Error(`${where} is not a list of property names`);
  }
  return [...value].sort();
}

// The properties and `required` list of each component schema with
// properties.
function documentedSchemas(document: unknown): Map<string, Members> {
  const components = isRecord(document) ? document.components : undefined;
  const schemas = isRecord(components) ? components.schemas : undefined;
  if (!isRecord(schemas)) {
    throw new Error("openapi.json has no components.schemas object");
  }
  const result = new Map<string, Members>();
  for (const [name, schema] of Object.entries(schemas)) {
    const where = `openapi.json components.schemas.${name}`;
    if (!isRecord(schema)) {
      throw new Error(`${where} is not a schema object`);
    }
    if (!("properties" in schema)) {
      continue;
    }
    if (!isRecord(schema.properties)) {
      throw new Error(`${where}.properties is not an object`);
    }
    result.set(name, {
      names: Object.keys(schema.properties).sort(),
      required:
        "required" in schema
          ? sortedNames(schema.required, `${where}.required`)
          : [],
    });
  }
  return result;
}

// The type of each member of `components.schemas` in the declaration file.
function declaredSchemas(source: string): Map<string, ts.TypeNode> {
  const file = ts.createSourceFile(
    "schema.d.ts",
    source,
    ts.ScriptTarget.Latest,
    false,
    ts.ScriptKind.TS,
  );
  const components = file.statements.find(
    (statement): statement is ts.InterfaceDeclaration =>
      ts.isInterfaceDeclaration(statement) &&
      statement.name.text === "components",
  );
  if (components === undefined) {
    throw new Error("schema.d.ts declares no `interface components`");
  }
  const schemas = components.members.find(
    (member): member is ts.PropertySignature =>
      ts.isPropertySignature(member) && memberName(member) === "schemas",
  );
  if (schemas?.type === undefined || !ts.isTypeLiteralNode(schemas.type)) {
    throw new Error(
      "schema.d.ts declares no `components.schemas` type literal",
    );
  }
  const result = new Map<string, ts.TypeNode>();
  for (const member of schemas.type.members) {
    if (!ts.isPropertySignature(member) || member.type === undefined) {
      throw new Error(
        `schema.d.ts components.schemas has a ${ts.SyntaxKind[member.kind]} member that is not a typed property`,
      );
    }
    result.set(memberName(member), member.type);
  }
  return result;
}

function memberName(member: ts.PropertySignature): string {
  const { name } = member;
  if (
    ts.isIdentifier(name) ||
    ts.isStringLiteral(name) ||
    ts.isNumericLiteral(name)
  ) {
    return name.text;
  }
  throw new Error(
    `schema.d.ts has a property named by a ${ts.SyntaxKind[name.kind]}`,
  );
}

// The members of the declared schema `name`: every property signature, and
// those without `?`. An index signature, which declares no property, is
// skipped.
function declaredMembers(
  declared: Map<string, ts.TypeNode>,
  name: string,
): Members {
  const where = `schema.d.ts components.schemas.${name}`;
  const type = declared.get(name);
  if (type === undefined) {
    throw new Error(`${where} is missing`);
  }
  if (!ts.isTypeLiteralNode(type)) {
    throw new Error(
      `${where} is a ${ts.SyntaxKind[type.kind]}, not an object type literal`,
    );
  }
  const names: string[] = [];
  const required: string[] = [];
  for (const member of type.members) {
    if (ts.isIndexSignatureDeclaration(member)) {
      continue;
    }
    if (!ts.isPropertySignature(member)) {
      throw new Error(
        `${where} has a ${ts.SyntaxKind[member.kind]} member that is not a property`,
      );
    }
    const property = memberName(member);
    names.push(property);
    if (member.questionToken === undefined) {
      required.push(property);
    }
  }
  return { names: names.sort(), required: required.sort() };
}

const documented = documentedSchemas(openapiDocument);
const declared = declaredSchemas(schemaSource);
const objectSchemaNames = [...documented.keys()].sort();

describe("schema.d.ts keeps the requiredness of openapi.json", () => {
  it("finds the object schemas of openapi.json", () => {
    expect(objectSchemaNames.length).toBeGreaterThan(0);
  });

  describe.each(objectSchemaNames)("components.schemas.%s", (name) => {
    it("declares exactly the documented properties", () => {
      expect(declaredMembers(declared, name).names, name).toEqual(
        documented.get(name)?.names,
      );
    });

    it("declares without `?` exactly the documented required properties", () => {
      expect(declaredMembers(declared, name).required, name).toEqual(
        documented.get(name)?.required,
      );
    });
  });

  it("types a PUT /accounts/{id} body carrying only the version token", () => {
    // Compile-time half: `tsc` rejects this declaration while any other
    // AccountUpdateRequest member is required, though the DTO accepts the body.
    const body: components["schemas"]["AccountUpdateRequest"] = {
      version: "0123456789abcdef",
    };
    expect(body).toEqual({ version: "0123456789abcdef" });
  });
});
