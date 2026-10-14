/** Locale-aware timestamp for display. `locale` is the UI language (i18n.language)
 *  — a bare toLocaleString() follows the OS locale instead, which disagrees with
 *  the app language the user picked. An unparseable value renders as nothing
 *  rather than the literal "Invalid Date". */
export function formatDateTime(value: string | number | Date, locale?: string): string {
  const d = value instanceof Date ? value : new Date(value);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleString(locale);
}
