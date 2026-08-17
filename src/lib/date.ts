const DATE_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/;

export function toLocalDateString(date = new Date()): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

export function parseLocalDate(value: string): Date {
  const match = DATE_PATTERN.exec(value);
  if (!match) {
    throw new Error(`Invalid local date: ${value}`);
  }
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const date = new Date(year, month - 1, day, 12, 0, 0, 0);
  if (
    date.getFullYear() !== year ||
    date.getMonth() !== month - 1 ||
    date.getDate() !== day
  ) {
    throw new Error(`Invalid local date: ${value}`);
  }
  return date;
}

export function addLocalDays(value: string, days: number): string {
  const date = parseLocalDate(value);
  date.setDate(date.getDate() + days);
  return toLocalDateString(date);
}

export function canNavigateForward(value: string, today = toLocalDateString()): boolean {
  return value < today;
}

export function nextLocalDate(value: string, today = toLocalDateString()): string {
  const candidate = addLocalDays(value, 1);
  return candidate > today ? today : candidate;
}

export function formatCalendarDate(value: string, today = toLocalDateString()): string {
  const date = parseLocalDate(value);
  const formatted = new Intl.DateTimeFormat('zh-CN', {
    month: 'long',
    day: 'numeric',
    weekday: 'short'
  }).format(date);
  return value === today ? `今天 · ${formatted}` : formatted;
}

