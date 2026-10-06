import { useCallback, useEffect, useState } from 'react';
import { request } from '../../utils/request';
import { useAccountStore } from '../../stores/useAccountStore';
import { isTauri } from '../../utils/env';
import { AppConfig } from '../../types/config';
import { formatCompactDuration } from '../../utils/liveLimit';

export type ModelLock = { model: string; until: number; detected_at: number; status: number; lock_type: string; transient_count: number; reason: string; message: string };
type LockStatistics = { locked: number; true_429: number; short_locks: number; expires_in_days: number[]; over_six_days: number; earliest_until?: number | null; latest_until?: number | null };
export type PoolStatus = {
    loaded_count: number; available_count: number; locked_account_count: number; locked_model_count: number;
    models: { model: string; loaded: number; available: number; locked: number; lock_stats?: LockStatistics }[];
    locked_accounts: { account_id: string; email?: string; loaded: boolean; quotas?: Record<string, number>; locks: Record<string, ModelLock> }[];
    server_time: number; storage_failed: boolean;
    runtime: { current_gb: number; limit_gb: number; in_flight: number; max_concurrent_requests: number; max_successes_per_minute: number; memory_pressure: boolean };
};

function formatLockRemaining(until: number, now: number): string {
    const seconds = Math.max(0, until - now);
    if (seconds < 86400) return formatCompactDuration(seconds);
    return `${Math.floor(seconds / 86400)}d ${Math.floor((seconds % 86400) / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

function LockCounts({ stats, now }: { stats?: LockStatistics; now: number }) {
    if (!stats) return <span className="text-gray-400">—</span>;
    const earliest = stats.earliest_until;
    const latest = stats.latest_until;
    const title = earliest && latest ? `最早到期：${new Date(earliest * 1000).toLocaleString()}；最晚到期：${new Date(latest * 1000).toLocaleString()}` : undefined;
    return <div className="text-xs whitespace-nowrap" title={title}>
        <p><span className={stats.true_429 ? 'text-orange-600' : 'text-gray-400'}>真429 {stats.true_429}</span><span className="mx-2 text-gray-400">/</span><span className={stats.short_locks ? 'text-amber-600' : 'text-gray-400'}>短锁 {stats.short_locks}</span></p>
        <p className="mt-1 text-gray-500 tabular-nums">{earliest && latest ? `到期 ${formatLockRemaining(earliest, now)}${latest !== earliest ? ` ～ ${formatLockRemaining(latest, now)}` : ''}` : '无活动锁'}</p>
    </div>;
}

function LockDays({ stats }: { stats?: LockStatistics }) {
    if (!stats) return <span className="text-gray-400">—</span>;
    return <div className="text-xs tabular-nums">
        <div className="grid grid-cols-3 gap-x-3 gap-y-1 whitespace-nowrap">{[6, 5, 4, 3, 2, 1].map(day => {
            const count = stats.expires_in_days[day - 1] ?? 0;
            return <span key={day} className={count ? 'text-gray-700 dark:text-gray-200' : 'text-gray-400'} title={`剩余时间大于 ${day - 1} 天且不超过 ${day} 天`}><span className="opacity-60">{day}d</span> <b>{count}</b></span>;
        })}</div>
        {stats.over_six_days > 0 && <p className="mt-1 text-gray-500">&gt;6d <b>{stats.over_six_days}</b></p>}
    </div>;
}
export function usePoolStatus(page = 0) {
    const [status, setStatus] = useState<PoolStatus>(); const [error, setError] = useState('');
    const refresh = useCallback(async () => {
        try { setStatus(await request<PoolStatus>('get_strict_pool_status', { page, includeDetails: false })); setError(''); }
        catch (e) { setError(String(e)); }
    }, [page]);
    useEffect(() => { let cancelled = false; let timer: ReturnType<typeof setTimeout>;
        const tick = async () => { await refresh(); if (!cancelled) timer = setTimeout(tick, 5000); };
        void tick(); return () => { cancelled = true; clearTimeout(timer); };
    }, [refresh]);
    return { status, error, refresh };
}

export default function PoolControl() {
    const { status, error, refresh } = usePoolStatus();
    const accounts = useAccountStore(s => s.accounts); const fetchAccounts = useAccountStore(s => s.fetchAccounts);
    const forbidden = accounts.filter(a => a.quota?.is_forbidden).length;
    const [successLimit, setSuccessLimit] = useState(1000); const [concurrent, setConcurrent] = useState(800); const [memory, setMemory] = useState(0);
    const [busy, setBusy] = useState(false); const [notice, setNotice] = useState('');
    useEffect(() => { request<AppConfig>('load_config').then(c => {
        setSuccessLimit(c.proxy.max_successes_per_minute ?? 1000); setConcurrent(c.proxy.max_concurrent_requests ?? 800); setMemory(c.proxy.max_memory_gb ?? 0);
    }).catch(e => setNotice(String(e))); }, []);
    async function save() {
        setBusy(true);
        try { const config = await request<AppConfig>('load_config');
            config.proxy.max_successes_per_minute = Math.max(1, Math.floor(successLimit));
            config.proxy.max_concurrent_requests = Math.max(1, Math.floor(concurrent));
            config.proxy.max_memory_gb = Math.max(0, memory);
            await request('save_config', { config }); setNotice('设置已保存并生效'); await refresh();
        } catch(e) { setNotice(String(e)); } finally { setBusy(false); }
    }
    async function exportDelete() {
        setBusy(true);
        try {
            const result = await request<{ accounts: unknown[]; deleted: number; backup_path?: string; errors?: string[] }>('export_delete_forbidden');
            if (result.accounts.length) {
                const content = JSON.stringify(result.accounts, null, 2);
                if (isTauri()) {
                    const { save } = await import('@tauri-apps/plugin-dialog');
                    const { invoke } = await import('@tauri-apps/api/core');
                    const path = await save({ defaultPath: `403-accounts-${new Date().toISOString().slice(0,10)}.json`, filters: [{ name: 'JSON', extensions: ['json'] }] });
                    if (path) await invoke('save_text_file', { path, content });
                } else {
                const blob = new Blob([content], { type: 'application/json' });
                const url = URL.createObjectURL(blob); const a = document.createElement('a'); a.href = url;
                a.download = `403-accounts-${new Date().toISOString().slice(0,10)}.json`; a.click(); setTimeout(() => URL.revokeObjectURL(url), 30000);
                }
            }
            setNotice(`已导出 ${result.accounts.length} 个、删除 ${result.deleted} 个 403 账号。${result.backup_path ? `备份：${result.backup_path}` : ''}${result.errors?.length ? `；部分删除失败：${result.errors.join('；')}` : ''}`);
            await fetchAccounts(); await refresh();
        } catch(e) { setNotice(String(e)); } finally { setBusy(false); }
    }
    const box = 'rounded-xl border border-gray-200 dark:border-base-300 bg-white dark:bg-base-100';
    const runtime = status?.runtime;
    const overview = [
        { label: '总账号', value: accounts.length },
        { label: '已加载', value: status?.loaded_count ?? '—' },
        { label: '实际可选', value: status?.available_count ?? '—', color: 'text-green-600' },
        { label: '挂锁账号 / 模型', value: `${status?.locked_account_count ?? '—'} / ${status?.locked_model_count ?? '—'}` },
        { label: '403 账号', value: forbidden, color: forbidden ? 'text-red-600' : '' },
        { label: '运行内存', value: runtime ? `${runtime.current_gb.toFixed(2)} GB` : '—' },
        { label: '处理中', value: runtime?.in_flight ?? '—' },
    ];
    return <section className="space-y-3" aria-label="账号池运行状态">
        <div className={`${box} grid grid-cols-2 sm:grid-cols-4 xl:grid-cols-7 gap-x-4 gap-y-3 p-4`}>
            {overview.map(item => <div key={item.label}>
                <p className="text-xs text-gray-500 dark:text-gray-400 mb-1">{item.label}</p>
                <strong className={`text-lg tabular-nums ${item.color ?? ''}`}>{item.value}</strong>
            </div>)}
        </div>
        <div className={`${box} overflow-x-auto`}>
            <table className="w-full text-sm" aria-label="独立模型可用账号数">
                <thead className="text-xs text-gray-500 dark:text-gray-400 border-b border-gray-100 dark:border-base-300">
                    <tr><th className="text-left p-3 font-medium">模型</th><th className="p-3 text-right font-medium">可用</th><th className="p-3 text-right font-medium">已加载冷却</th><th className="p-3 text-right font-medium">已加载且支持</th><th className="p-3 text-left font-medium">全部锁 · 类型与时间</th><th className="p-3 text-left font-medium">到期分布</th></tr>
                </thead>
                <tbody>{status?.models.map(m => <tr key={m.model} className="border-b last:border-0 border-gray-100 dark:border-base-300/50">
                    <td className="px-3 py-2 font-mono text-xs whitespace-nowrap">{m.model}</td>
                    <td className="px-3 py-2 text-right font-semibold text-green-600 tabular-nums">{m.available}</td>
                    <td className={`px-3 py-2 text-right tabular-nums ${m.locked ? 'text-orange-600' : 'text-gray-400'}`}>{m.locked}</td>
                    <td className="px-3 py-2 text-right tabular-nums">{m.loaded}</td>
                    <td className="px-3 py-2"><LockCounts stats={m.lock_stats} now={status?.server_time ?? 0} /></td>
                    <td className="px-3 py-2"><LockDays stats={m.lock_stats} /></td>
                </tr>)}</tbody>
            </table>
            {!status && <p className="p-3 text-sm text-gray-500">正在读取模型状态…</p>}
            {status && <p className="px-3 py-2 text-xs text-gray-500 border-t border-gray-100 dark:border-base-300">短锁含假429与503；全部锁含未加载账号。1d 表示24小时内，2d 表示24–48小时，依此类推。按当前有效锁统计，不提前解锁。</p>}
        </div>
        <details className={box}>
            <summary className="p-3 cursor-pointer text-sm font-medium">运行设置与账号维护</summary>
            <div className="p-4 pt-1 flex flex-wrap items-end gap-4">
            <label className="text-sm">每分钟成功上限<input aria-label="每分钟成功上限" className="input input-bordered input-sm block w-32" type="number" min="1" value={successLimit} onChange={e => setSuccessLimit(Number(e.target.value) || 1)} /></label>
            <label className="text-sm">最大并发<input aria-label="最大并发" className="input input-bordered input-sm block w-32" type="number" min="1" value={concurrent} onChange={e => setConcurrent(Number(e.target.value) || 1)} /></label>
            <label className="text-sm">内存阈值 GB（0 不限制）<input aria-label="内存阈值 GB" className="input input-bordered input-sm block w-32" type="number" min="0" step="0.5" value={memory} onChange={e => setMemory(Number(e.target.value) || 0)} /></label>
            <button className="btn btn-primary btn-sm" disabled={busy} onClick={save}>保存</button>
            <button disabled={busy || !forbidden} onClick={exportDelete} className="btn btn-error btn-sm">导出并删除 {forbidden} 个 403 账号</button>
            <p className="w-full text-xs opacity-60">内存达到阈值时暂停接收新请求，已有请求继续完成。可用数按当前后端调度条件计算。</p>
            </div>
        </details>
        {(notice || error) && <p role="status" className="text-sm break-all">{notice || error}</p>}
        {status?.storage_failed && <p role="alert" className="text-error">模型锁存储异常，已阻止选号；请检查磁盘及服务日志后重启，避免丢失冷却状态。</p>}
    </section>;
}
