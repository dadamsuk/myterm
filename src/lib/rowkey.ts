/** The session key of a row key: "host/slug:@3" -> "host/slug". */
export function sessionKeyOfRow(row: string): string {
  const i = row.lastIndexOf(":@");
  return i < 0 ? row : row.slice(0, i);
}
