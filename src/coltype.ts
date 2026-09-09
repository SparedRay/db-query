// What kind of thing a column holds, from the name of its type.
//
// # Why this is a guess, and why that is fine
//
// The tree gets `dataType` as the server spells it — `varchar(190)`, `int`,
// `decimal(12,2)`, `json`, or, on Elasticsearch, `keyword` and `half_float`.
// There is no cross-engine enumeration of those, so this is a lookup table over
// names, and anything it does not recognise falls to `other` and keeps the
// plain dot the tree has always drawn.
//
// The consequence of a wrong answer is one misleading 14px icon beside a type
// name that is *also* on screen. That is the whole reason it is allowed to
// guess: the icon is a glance, the text beside it is the truth.
//
// The vocabulary is deliberately the same as Rust's `TypeHint`
// (`decode.rs`) — which classifies *result* columns for alignment — with
// `object` added, because JSON is the one shape the grid never needed to tell
// apart and the tree does.

import type { IconName } from "./icons";

export type TypeFamily =
  | "numeric"
  | "text"
  | "temporal"
  | "bool"
  | "binary"
  | "object"
  | "other";

/**
 * The type name without its parameters: `decimal(12,2)` → `decimal`.
 *
 * Also drops trailing words like `unsigned` and `zerofill`, which qualify a
 * type rather than naming one. `double precision` loses "precision" the same
 * way and lands on `double`, which is the answer.
 */
function base(dataType: string): string {
  return dataType.trim().toLowerCase().split(/[\s(]/, 1)[0] ?? "";
}

const FAMILIES: Record<string, TypeFamily> = {};
const put = (family: TypeFamily, names: string) => {
  for (const n of names.split(" ")) FAMILIES[n] = family;
};

put(
  "numeric",
  // MySQL, then the names Elasticsearch uses for the same idea.
  "tinyint smallint mediumint int integer bigint decimal dec numeric fixed" +
    " float double real serial" +
    " long short byte half_float scaled_float unsigned_long number",
);
put("bool", "bool boolean");
// `bit` is a bit *string* — MySQL hands it over as bytes, and that is what the
// grid shows — so it belongs with the blobs rather than with the booleans,
// which is where `bit(1)` tempts you to put it.
put("binary", "binary varbinary blob tinyblob mediumblob longblob bit geometry");
put("temporal", "date time datetime timestamp year date_nanos");
put(
  "text",
  "char varchar tinytext text mediumtext longtext enum set" +
    " keyword constant_keyword wildcard match_only_text version ip uuid",
);
// `geo_point` and `geo_shape` are structured documents in Elasticsearch, not
// the byte blobs MySQL's spatial types are.
put("object", "json object nested flattened geo_point geo_shape");

/** Which family a column's declared type belongs to. */
export function typeFamily(dataType: string): TypeFamily {
  return FAMILIES[base(dataType)] ?? "other";
}

/**
 * Which icon stands for a column of this type.
 *
 * `other` keeps the plain dot the tree drew before any of this existed, so an
 * unrecognised type looks like a column rather than like a wrong guess.
 */
const ICONS: Record<TypeFamily, IconName> = {
  numeric: "type-numeric",
  text: "type-text",
  temporal: "type-temporal",
  bool: "type-bool",
  binary: "type-binary",
  object: "type-object",
  other: "column",
};

export function columnIcon(dataType: string): IconName {
  return ICONS[typeFamily(dataType)];
}
