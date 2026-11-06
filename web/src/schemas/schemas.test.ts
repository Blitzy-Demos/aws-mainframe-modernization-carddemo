// OpenAPI conformance of the request-body form schemas (AAP 0.5.4.3). The committed
// contract, openapi/openapi.json, is the authority: for every request body DTO it
// declares, the zod schema its form submits with (the registry in ./index.ts) must
// accept a valid entry taken from the fixture corpus, submit a body that meets every
// published maxLength and pattern, and reject, on the offending property, a value one
// character over a maxLength, a value outside a pattern and the absence of a required
// member. The registry must name exactly the DTOs the document's request bodies refer
// to, and every operation queryFormSchemas is keyed by must still be in the document.
//
// The document is read through runtime guards instead of its inferred JSON type, so a
// shape this test does not expect fails with its location rather than being skipped.
import { describe, expect, it } from "vitest";
import type { z } from "zod";

import openapi from "../../openapi/openapi.json";

import {
  REQUEST_DTO_NAMES,
  queryFormSchemas,
  requestSchemas,
  type RequestDtoName,
  type RequestSchemas,
} from "./index";

// ---------------------------------------------------------------------------------------
// The parts of the OpenAPI document this test reads
// ---------------------------------------------------------------------------------------

// A property schema, or a schema a property refers to.
interface PropertySchema {
  readonly type?: string | readonly string[];
  readonly maxLength?: number;
  readonly pattern?: string;
  readonly $ref?: string;
  readonly oneOf?: readonly PropertySchema[];
  readonly anyOf?: readonly PropertySchema[];
  readonly allOf?: readonly PropertySchema[];
  readonly enum?: readonly unknown[];
}

// A component schema, with its members when it describes an object.
interface ObjectSchema extends PropertySchema {
  readonly properties?: ReadonlyMap<string, PropertySchema>;
  readonly required?: readonly string[];
}

interface MediaTypeSchema {
  readonly schema?: PropertySchema;
}

interface RequestBodySchema {
  // Media type → its schema. Empty when the body declares no content.
  readonly content: ReadonlyMap<string, MediaTypeSchema>;
}

interface OperationSchema {
  readonly requestBody?: RequestBodySchema;
}

interface OpenApiDoc {
  // Path → lower-case HTTP method → operation.
  readonly paths: ReadonlyMap<string, ReadonlyMap<string, OperationSchema>>;
  readonly components: { readonly schemas: ReadonlyMap<string, ObjectSchema> };
}

type JsonObject = { readonly [key: string]: unknown };

// The fields of a path item that are operations; `parameters` and the like are not.
const HTTP_METHODS: ReadonlySet<string> = new Set([
  "get",
  "put",
  "post",
  "delete",
  "options",
  "head",
  "patch",
  "trace",
]);

const COMPONENT_REF = "#/components/schemas/";

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function objectAt(value: unknown, where: string): JsonObject {
  if (!isObject(value)) {
    throw new Error(`openapi.json ${where} is not an object`);
  }
  return value;
}

function arrayAt(value: unknown, where: string): readonly unknown[] {
  if (!Array.isArray(value)) {
    throw new Error(`openapi.json ${where} is not an array`);
  }
  return value;
}

function stringAt(value: unknown, where: string): string {
  if (typeof value !== "string") {
    throw new Error(`openapi.json ${where} is not a string`);
  }
  return value;
}

function lengthAt(value: unknown, where: string): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    throw new Error(`openapi.json ${where} is not a non-negative integer`);
  }
  return value;
}

function stringsAt(value: unknown, where: string): readonly string[] {
  return arrayAt(value, where).map((item, index) => stringAt(item, `${where}[${index}]`));
}

function typeAt(value: unknown, where: string): string | readonly string[] {
  return Array.isArray(value) ? stringsAt(value, where) : stringAt(value, where);
}

// `object[key]` read by `read` when present, `undefined` when absent.
function member<T>(
  object: JsonObject,
  key: string,
  where: string,
  read: (value: unknown, where: string) => T,
): T | undefined {
  return key in object ? read(object[key], `${where}.${key}`) : undefined;
}

// Entries of a JSON object, each value read by `read`, as a map.
function mapAt<T>(
  value: unknown,
  where: string,
  read: (value: unknown, where: string) => T,
): ReadonlyMap<string, T> {
  return new Map(
    Object.entries(objectAt(value, where)).map(([key, entry]): [string, T] => [
      key,
      read(entry, `${where}["${key}"]`),
    ]),
  );
}

function schemasAt(value: unknown, where: string): readonly PropertySchema[] {
  return arrayAt(value, where).map((item, index) => propertyAt(item, `${where}[${index}]`));
}

function propertyAt(value: unknown, where: string): PropertySchema {
  const object = objectAt(value, where);
  return {
    type: member(object, "type", where, typeAt),
    maxLength: member(object, "maxLength", where, lengthAt),
    pattern: member(object, "pattern", where, stringAt),
    $ref: member(object, "$ref", where, stringAt),
    oneOf: member(object, "oneOf", where, schemasAt),
    anyOf: member(object, "anyOf", where, schemasAt),
    allOf: member(object, "allOf", where, schemasAt),
    enum: member(object, "enum", where, arrayAt),
  };
}

function objectSchemaAt(value: unknown, where: string): ObjectSchema {
  const object = objectAt(value, where);
  return {
    ...propertyAt(object, where),
    properties: member(object, "properties", where, (properties, at) =>
      mapAt(properties, at, propertyAt),
    ),
    required: member(object, "required", where, stringsAt),
  };
}

function mediaTypeAt(value: unknown, where: string): MediaTypeSchema {
  return { schema: member(objectAt(value, where), "schema", where, propertyAt) };
}

function requestBodyAt(value: unknown, where: string): RequestBodySchema {
  const content = member(objectAt(value, where), "content", where, (media, at) =>
    mapAt(media, at, mediaTypeAt),
  );
  return { content: content ?? new Map() };
}

function operationAt(value: unknown, where: string): OperationSchema {
  return {
    requestBody: member(objectAt(value, where), "requestBody", where, requestBodyAt),
  };
}

function pathItemAt(value: unknown, where: string): ReadonlyMap<string, OperationSchema> {
  return new Map(
    Object.entries(objectAt(value, where))
      .filter(([field]) => HTTP_METHODS.has(field))
      .map(([method, operation]): [string, OperationSchema] => [
        method,
        operationAt(operation, `${where}.${method}`),
      ]),
  );
}

// The paths and component schemas of the document, checked as they are read.
function readDoc(raw: unknown): OpenApiDoc {
  const root = objectAt(raw, "document");
  const components = objectAt(root.components, "components");
  return {
    paths: mapAt(root.paths, "paths", pathItemAt),
    components: {
      schemas: mapAt(components.schemas, "components.schemas", objectSchemaAt),
    },
  };
}

const doc = readDoc(openapi);

// The component name a `#/components/schemas/<Name>` reference names.
function componentName(ref: string): string | undefined {
  return ref.startsWith(COMPONENT_REF) ? ref.slice(COMPONENT_REF.length) : undefined;
}

// The schema whose `maxLength` and `pattern` govern a property: a `$ref` is followed
// into `components.schemas`, and of a `oneOf`, `anyOf` or `allOf` the one member that
// is not `null` is taken. Any other shape throws with its location, so the reader is
// extended rather than a constraint silently missed.
function resolveProperty(
  property: PropertySchema,
  where: string,
  followed: readonly string[] = [],
): PropertySchema {
  if (property.$ref !== undefined) {
    const name = componentName(property.$ref);
    if (name === undefined || followed.includes(name)) {
      throw new Error(`openapi.json ${where}: cannot follow $ref ${property.$ref}`);
    }
    const target = doc.components.schemas.get(name);
    if (target === undefined) {
      throw new Error(`openapi.json ${where}: $ref names no schema ${name}`);
    }
    return resolveProperty(target, `components.schemas.${name}`, [...followed, name]);
  }
  for (const keyword of ["oneOf", "anyOf", "allOf"] as const) {
    const members = property[keyword];
    if (members === undefined) {
      continue;
    }
    const effective = members.filter((schema) => schema.type !== "null");
    if (effective.length !== 1) {
      throw new Error(
        `openapi.json ${where}.${keyword} has ${effective.length} non-null members, not one`,
      );
    }
    return resolveProperty(effective[0], `${where}.${keyword}`, followed);
  }
  return property;
}

// ---------------------------------------------------------------------------------------
// The request body contract of each DTO
// ---------------------------------------------------------------------------------------

// The published constraints of one request body property, read from its effective schema.
interface PropertyContract {
  readonly name: string;
  readonly maxLength?: number;
  readonly pattern?: string;
}

interface DtoContract {
  readonly properties: readonly PropertyContract[];
  readonly required: readonly string[];
}

// The contract of the component object schema `name`; `undefined` when it has none.
function contractOf(name: string): DtoContract | undefined {
  const schema = doc.components.schemas.get(name);
  if (schema?.properties === undefined) {
    return undefined;
  }
  return {
    properties: [...schema.properties].map(([property, declared]): PropertyContract => {
      const { maxLength, pattern } = resolveProperty(
        declared,
        `components.schemas.${name}.properties.${property}`,
      );
      return { name: property, maxLength, pattern };
    }),
    required: schema.required ?? [],
  };
}

// Each operation with a request body, and the DTO its JSON body refers to, if any.
const requestBodies = [...doc.paths].flatMap(([path, operations]) =>
  [...operations].flatMap(([method, operation]) => {
    if (operation.requestBody === undefined) {
      return [];
    }
    const ref = operation.requestBody.content.get("application/json")?.schema?.$ref;
    return [
      {
        operation: `${method.toUpperCase()} ${path}`,
        dto: ref === undefined ? undefined : componentName(ref),
      },
    ];
  }),
);

// JSON Schema counts a string's length in code points, not UTF-16 code units.
function codePoints(value: string): number {
  return [...value].length;
}

// ---------------------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------------------

// A token of the shape `GET` returns for `version`: 16 lower-case hex digits.
const VERSION = "0f1e2d3c4b5a6978";

// One entry of each form that every client edit accepts, typed by the form's input so a
// form or DTO change fails `tsc` here. Values are fixture records within the widths of
// their BMS fields: account 00000000001 [app/data/ASCII/acctdata.txt:L1], its customer
// 000000001 [app/data/ASCII/custdata.txt:L1] and card 9680294154603697
// [app/data/ASCII/carddata.txt:L49], the merchant of the first daily transaction
// [app/data/ASCII/dailytran.txt:L1], the DUSRSECJ users ADMIN001 and USER0001
// [app/jcl/DUSRSECJ.jcl:L35, L40], and the type and category of the retail reward seed
// rule. Each fills every member its DTO declares, so each published constraint is met
// by a real value.
const validSamples: { [K in RequestDtoName]: z.input<RequestSchemas[K]> } = {
  LoginRequest: { user_id: "ADMIN001", password: "PASSWORD" },
  AccountUpdateRequest: {
    version: VERSION,
    account_id: "00000000001",
    acct_active_status: "Y",
    acct_open_year: "2014",
    acct_open_mon: "11",
    acct_open_day: "20",
    acct_credit_limit: "2020.00",
    acct_exp_year: "2025",
    acct_exp_mon: "05",
    acct_exp_day: "20",
    acct_cash_credit_limit: "1020.00",
    acct_reissue_year: "2025",
    acct_reissue_mon: "05",
    acct_reissue_day: "20",
    acct_curr_bal: "194.00",
    acct_curr_cyc_credit: "0.00",
    // ACCT-GROUP-ID is blank in every fixture account (E-06).
    acct_group_id: "",
    acct_curr_cyc_debit: "0.00",
    customer_id: "000000001",
    cust_ssn_1: "020",
    cust_ssn_2: "97",
    cust_ssn_3: "3888",
    cust_dob_year: "1961",
    cust_dob_mon: "06",
    cust_dob_day: "08",
    cust_fico_credit_score: "274",
    cust_first_name: "Immanuel",
    cust_middle_name: "Madeline",
    cust_last_name: "Kessler",
    cust_addr_line_1: "618 Deshaun Route",
    cust_addr_state_cd: "NC",
    cust_addr_line_2: "Apt. 802",
    cust_addr_zip: "12546",
    cust_addr_line_3: "Altenwerthshire",
    cust_addr_country_cd: "USA",
    cust_phone_num_1a: "908",
    cust_phone_num_1b: "119",
    cust_phone_num_1c: "8310",
    cust_govt_issued_id: "00000000000049368437",
    cust_phone_num_2a: "373",
    cust_phone_num_2b: "693",
    cust_phone_num_2c: "8684",
    cust_eft_account_id: "0053581756",
    cust_pri_card_holder_ind: "Y",
  },
  CardUpdateRequest: {
    version: VERSION,
    account_id: "00000000001",
    card_number: "9680294154603697",
    embossed_name: "Immanuel Kessler",
    active_status: "Y",
    expiry_month: "05",
    expiry_year: "2025",
  },
  TransactionAddRequest: {
    account_id: "00000000001",
    card_number: "9680294154603697",
    tran_type_cd: "01",
    tran_cat_cd: "0001",
    tran_source: "POS TERM",
    tran_desc: "Purchase at Abshire-Lowe",
    tran_amt: "+00000010.00",
    tran_orig_date: "2022-07-01",
    tran_proc_date: "2022-07-01",
    tran_merchant_id: "000000001",
    tran_merchant_name: "Abshire-Lowe",
    tran_merchant_city: "North Enoshaven",
    tran_merchant_zip: "72112",
    confirm: "Y",
  },
  PaymentRequest: { account_id: "00000000001", confirm: "Y" },
  UserAddRequest: {
    first_name: "Test",
    last_name: "User",
    user_id: "TEST0001",
    password: "PASSWORD",
    usr_type: "U",
  },
  UserUpdateRequest: {
    user_id: "USER0001",
    first_name: "LAWRENCE",
    last_name: "THOMAS",
    change_password: true,
    password: "PASSWORD",
    usr_type: "U",
  },
  TranTypeCreateRequest: { type_code: "08", description: "Test type" },
  TranTypeUpdateRequest: { description: "PURCHASE" },
  RewardRuleRequest: {
    transaction_type_cd: "01",
    transaction_category_cd: "0001",
    reward_rate: "1.0000",
    min_transaction_amount: "10.00",
    max_transaction_amount: "500.00",
    active_from: "2022-01-01",
    active_to: "2030-01-01",
  },
};

// Values tried, in order, as a pattern violation: the first the pattern rejects and the
// property's maxLength admits is used. The euro sign (U+20AC) ends the list because the
// free-text, user and reward type-code patterns admit every ASCII character, and the
// type code every Latin-1 one, so only a character above U+00FF violates them. A
// pattern none of these violates fails its test, so the list is extended rather than
// the check skipped.
const PATTERN_VIOLATIONS: readonly string[] = ["!", "x", "-", "0", "Z", "a b", "~~", "\u20ac"];

function violationOf({ maxLength, pattern }: PropertyContract): string | undefined {
  if (pattern === undefined) {
    return undefined;
  }
  const published = new RegExp(pattern, "u");
  return PATTERN_VIOLATIONS.find(
    (candidate) =>
      !published.test(candidate) &&
      (maxLength === undefined || codePoints(candidate) <= maxLength),
  );
}

type Entry = Readonly<Record<string, unknown>>;

function issuesOf(schema: z.ZodTypeAny, entry: Entry): readonly z.ZodIssue[] {
  const result = schema.safeParse(entry);
  return result.success ? [] : result.error.issues;
}

// The issues of `entry` that name `property` as the failing member.
function issuesAt(schema: z.ZodTypeAny, entry: Entry, property: string): readonly z.ZodIssue[] {
  return issuesOf(schema, entry).filter((issue) => issue.path[0] === property);
}

// Each way the parsed body `body` departs from the published maxLength and pattern.
function contractViolations(contract: DtoContract, body: unknown): string[] {
  if (!isObject(body)) {
    return ["the parsed body is not an object"];
  }
  return contract.properties.flatMap(({ name, maxLength, pattern }) => {
    if (maxLength === undefined && pattern === undefined) {
      return [];
    }
    const value = body[name];
    if (typeof value !== "string") {
      return [`${name} is not a string in the body`];
    }
    const violations: string[] = [];
    if (maxLength !== undefined && codePoints(value) > maxLength) {
      violations.push(`${name} ${JSON.stringify(value)} is over maxLength ${maxLength}`);
    }
    if (pattern !== undefined && !new RegExp(pattern, "u").test(value)) {
      violations.push(`${name} ${JSON.stringify(value)} does not match ${pattern}`);
    }
    return violations;
  });
}

// ---------------------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------------------

describe("the request body registry", () => {
  it("finds the DTO of every request body", () => {
    const unnamed = requestBodies
      .filter(({ dto }) => dto === undefined)
      .map(({ operation }) => operation);
    expect(unnamed, "request bodies without an application/json $ref to a component").toEqual(
      [],
    );
  });

  it("registers exactly the DTOs the request bodies refer to", () => {
    const bodyDtos = [
      ...new Set(requestBodies.flatMap(({ dto }) => (dto === undefined ? [] : [dto]))),
    ].sort();
    expect(bodyDtos.length, "request body DTOs in openapi.json").toBeGreaterThan(0);
    expect([...REQUEST_DTO_NAMES].sort(), "REQUEST_DTO_NAMES").toEqual(bodyDtos);
    expect(Object.keys(requestSchemas).sort(), "requestSchemas").toEqual(bodyDtos);
  });
});

describe.each([...REQUEST_DTO_NAMES])("%s", (name) => {
  const schema: z.ZodTypeAny = requestSchemas[name];
  const sample: Entry = validSamples[name];
  const contract = contractOf(name);

  it("is an object schema of openapi.json", () => {
    expect(contract, `components.schemas.${name} with properties`).toBeDefined();
  });
  if (contract === undefined) {
    return;
  }

  it("accepts the fixture entry", () => {
    expect(issuesOf(schema, sample)).toEqual([]);
  });

  // The parsed values are checked, not the entry: they are what the form submits, and a
  // form may transform an entry (the reward-rule dates become RFC 3339 instants).
  it("submits a body within every published maxLength and pattern", () => {
    const result = schema.safeParse(sample);
    const body: unknown = result.data;
    expect(contractViolations(contract, body)).toEqual([]);
  });

  for (const { name: property, maxLength } of contract.properties) {
    if (maxLength === undefined) {
      continue;
    }
    it(`rejects ${property} one character over maxLength ${maxLength}`, () => {
      const entry = { ...sample, [property]: "9".repeat(maxLength + 1) };
      expect(
        issuesAt(schema, entry, property).length,
        `${name}.${property} accepts ${maxLength + 1} characters`,
      ).toBeGreaterThan(0);
    });
  }

  for (const property of contract.properties) {
    if (property.pattern === undefined) {
      continue;
    }
    it(`rejects ${property.name} outside its pattern`, () => {
      const violation = violationOf(property);
      expect(
        violation,
        `no PATTERN_VIOLATIONS candidate breaks ${name}.${property.name} pattern ` +
          `${property.pattern} within its maxLength`,
      ).toBeDefined();
      const entry = { ...sample, [property.name]: violation };
      expect(
        issuesAt(schema, entry, property.name).length,
        `${name}.${property.name} accepts ${JSON.stringify(violation)}`,
      ).toBeGreaterThan(0);
    });
  }

  for (const required of contract.required) {
    it(`rejects an entry without the required ${required}`, () => {
      expect(sample, `the ${name} entry`).toHaveProperty(required);
      const { [required]: _omitted, ...entry } = sample;
      expect(
        issuesAt(schema, entry, required).length,
        `${name} accepts an entry without ${required}`,
      ).toBeGreaterThan(0);
    });
  }
});

describe("queryFormSchemas", () => {
  it.each(Object.keys(queryFormSchemas))("%s is an operation of openapi.json", (key) => {
    const [method, path, ...rest] = key.split(" ");
    expect(rest, `${key} as "<METHOD> <path>"`).toEqual([]);
    const operation = method.toLowerCase();
    expect(
      doc.paths.get(path)?.has(operation),
      `openapi.json paths["${path}"].${operation}`,
    ).toBe(true);
  });
});

