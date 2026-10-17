import { useEffect, useState } from 'react';
import type { Account } from '../../types/account';
import { formatCompactDuration, getLiveLimitForModel } from '../../utils/liveLimit';
import { INDEPENDENT_MODELS, findIndependentQuota } from '../../utils/independentModels';

export default function IndependentQuotas({ account }: { account: Account }) {
    const [now, setNow] = useState(Math.floor(Date.now() / 1000));
    const active = Object.values(account.live_limited_models ?? {}).some(v => v.until > now);
    useEffect(() => { if (!active) return; const timer = setInterval(() => setNow(Math.floor(Date.now()/1000)), 1000); return () => clearInterval(timer); }, [active]);
    return <details className="mt-2 text-xs" open={active || undefined} onClick={e => e.stopPropagation()}>
        <summary className="cursor-pointer text-blue-600">独立模型额度{active ? ' · 存在活动锁' : ''}</summary>
        <div className="grid gap-1 mt-1">{INDEPENDENT_MODELS.map(model => {
            const quota = findIndependentQuota(account.quota?.models, model);
            const lock = getLiveLimitForModel(account, model); const left = Math.max(0, (lock?.until ?? 0) - now);
            return <div key={model} className="flex flex-wrap justify-between gap-x-2" title={left ? lock?.reason : quota ? `额度来源：${quota.name}` : '上游未返回该模型额度'}>
                <span className="font-mono">{model}</span><span>{quota ? `${quota.percentage}%` : '无额度数据'}{left > 0 && <b className="text-orange-600 ml-1">锁定 {formatCompactDuration(left)}</b>}</span>
            </div>;
        })}</div>
    </details>;
}
