export function formatDuration(activeMs: number): string {
  const safeMs = Math.max(0, Math.round(activeMs));
  if (safeMs === 0) {
    return '0 分钟';
  }
  if (safeMs < 60_000) {
    return `${Math.max(1, Math.round(safeMs / 1_000))} 秒`;
  }

  const totalMinutes = Math.floor(safeMs / 60_000);
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours === 0) {
    return `${totalMinutes} 分钟`;
  }
  return minutes === 0 ? `${hours} 小时` : `${hours} 小时 ${minutes} 分钟`;
}

export function formatPercentage(share: number): string {
  const safeShare = Number.isFinite(share) ? Math.min(1, Math.max(0, share)) : 0;
  return new Intl.NumberFormat('zh-CN', {
    style: 'percent',
    maximumFractionDigits: 1
  }).format(safeShare);
}

