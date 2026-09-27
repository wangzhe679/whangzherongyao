import type { ModelQuota, QuotaGroup } from '../types/account';

// Grouped usage windows are informational; only explicit model cooldowns block.
export function getModelQuotaDisplay(_modelId: string, model: ModelQuota | undefined, _groups: QuotaGroup[] = []) {
    return { percentage: model?.percentage ?? 0, resetTime: model?.reset_time,
        isWeeklyConstrained: false, weeklyResetTime: undefined };
}
