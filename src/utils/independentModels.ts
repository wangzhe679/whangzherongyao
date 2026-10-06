export const INDEPENDENT_MODELS = [
    'claude-sonnet-4-6',
    'claude-opus-4-6-thinking',
    'gemini-3.6-flash-tiered',
    'gemini-3.7-flash-tiered',
    'gemini-3.8-flash-tiered',
    'gemini-3.1-pro-low',
    'gemini-3.1-flash-lite',
] as const;

export function independentModelKey(model: string): string {
    const key = model.trim().toLowerCase().replace(/^models\//, '').replace(/^\[思考\]/, '');
    if (key === 'claude-opus-4-6') return 'claude-opus-4-6-thinking';
    if (key === 'claude-sonnet-4-6-thinking') return 'claude-sonnet-4-6';
    const flash = /^(gemini-3\.[678]-flash)(?:-(?:high|medium|low|tiered))?$/.exec(key);
    return flash ? `${flash[1]}-tiered` : key;
}

export function findIndependentQuota<T extends { name: string }>(models: T[] | undefined, model: string): T | undefined {
    return models?.find(m => m.name.toLowerCase() === model)
        ?? models?.find(m => independentModelKey(m.name) === model)
        // The backend already treats the Pro high/low quota names as aliases.
        ?? (model === 'gemini-3.1-pro-low' ? models?.find(m => m.name === 'gemini-3.1-pro-high') : undefined);
}
