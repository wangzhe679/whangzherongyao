export const INDEPENDENT_MODELS = [
    'claude-sonnet-4-6',
    'claude-opus-4-6-thinking',
    'gemini-3.8-flash-tiered',
    'gemini-3.1-flash-lite',
    'gemini-4-argon',
] as const;

export function independentModelKey(model: string): string {
    const key = model.trim().toLowerCase().replace(/^models\//, '').replace(/^\[思考\]/, '');
    if (key === 'claude-opus-4-6') return 'claude-opus-4-6-thinking';
    if (key === 'claude-sonnet-4-6-thinking') return 'claude-sonnet-4-6';
    return key;
}

export function findIndependentQuota<T extends { name: string }>(models: T[] | undefined, model: string): T | undefined {
    return models?.find(m => m.name.toLowerCase() === model)
        ?? models?.find(m => independentModelKey(m.name) === model);
}