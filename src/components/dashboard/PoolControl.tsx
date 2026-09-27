import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { request } from '../../utils/request';
import { useAccountStore } from '../../stores/useAccountStore';
import { isTauri } from '../../utils/env';
import { AppConfig } from '../../types/config';

export type ModelLock = { model: string; until: number; detected_at: number; status: number; lock_type: string; transient_count: number; reason: string; message: string };
export type PoolStatus = {
    loaded_count: number; available_count: number; locked_account_count: number; locked_model_count: number;
    models: { model: string; loaded: number; available: number; locked: number }[];
    locked_accounts: { account_id: string; email?: string; loaded: boolean; quotas?: Record<string, number>; locks: Record<string, ModelLock> }[];
    server_time: number; storage_failed: boolean;
    runtime: { current_gb: number; limit_gb: number; in_flight: number; max_concurrent_requests: number; max_successes_per_minute: number; memory_pressure: boolean };
};
export function usePoolStatus(page = 0) {
    const [status, setStatus] = useState<PoolStatus>(); const [error, setError] = useState('');
    const refresh = useCallback(async () => {
        try { setStatus(await request<PoolStatus>('get_strict_pool_status', { page })); setError(''); }
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
    const box = 'rounded-xl border border-gray-200 dark:border-base-300 bg-white dark:bg-base-100 p-4';
    return <section className="space-y-3" aria-label="账号池运行状态">
        <div className="grid grid-cols-2 lg:grid-cols-5 gap-3">
            <div className={box}><p className="text-sm opacity-60">已加载账号</p><strong className="text-2xl">{status?.loaded_count ?? '—'}</strong></div>
            <div className={box}><p className="text-sm opacity-60">实际可选账号</p><strong className="text-2xl text-green-600">{status?.available_count ?? '—'}</strong></div>
            <Link to="/model-locks" className={box}><p className="text-sm opacity-60">挂锁账号 / 模型</p><strong className="text-2xl">{status?.locked_account_count ?? '—'} / {status?.locked_model_count ?? '—'}</strong><p className="text-xs text-blue-600">查看全部锁与倒计时 →</p></Link>
            <div className={box}><p className="text-sm opacity-60">403 账号</p><strong className="text-2xl text-red-600">{forbidden}</strong><button disabled={busy || !forbidden} onClick={exportDelete} className="btn btn-error btn-xs ml-2">一键导出＋删除</button></div>
            <div className={box}><p className="text-sm opacity-60">内存 / 正在处理</p><strong>{status?.runtime.current_gb.toFixed(2) ?? '—'} GB / {status?.runtime.in_flight ?? '—'}</strong></div>
        </div>
        <div className="grid grid-cols-2 xl:grid-cols-3 gap-3">{status?.models.map(m => <div className={box} key={m.model}><p className="text-xs break-all">{m.model.startsWith('claude') ? 'Claude 统一模型组' : m.model}</p><p><b className="text-green-600">{m.available}</b> 可用 · {m.locked} 冷却 · {m.loaded} 已加载且支持</p></div>)}</div>
        <div className={`${box} flex flex-wrap items-end gap-4`}>
            <label className="text-sm">每分钟成功上限<input aria-label="每分钟成功上限" className="input input-bordered input-sm block w-32" type="number" min="1" value={successLimit} onChange={e => setSuccessLimit(Number(e.target.value) || 1)} /></label>
            <label className="text-sm">最大并发<input aria-label="最大并发" className="input input-bordered input-sm block w-32" type="number" min="1" value={concurrent} onChange={e => setConcurrent(Number(e.target.value) || 1)} /></label>
            <label className="text-sm">内存阈值 GB（0 不限制）<input aria-label="内存阈值 GB" className="input input-bordered input-sm block w-32" type="number" min="0" step="0.5" value={memory} onChange={e => setMemory(Number(e.target.value) || 0)} /></label>
            <button className="btn btn-primary btn-sm" disabled={busy} onClick={save}>保存</button>
            <p className="text-xs opacity-60">内存达到阈值时暂停接收新请求；已有请求继续完成。可用数按后端调度条件计算，上游实际结果仍以请求为准。</p>
        </div>
        {(notice || error) && <p role="status" className="text-sm break-all">{notice || error}</p>}
        {status?.storage_failed && <p role="alert" className="text-error">模型锁存储异常，已阻止选号；请检查磁盘及服务日志后重启，避免丢失冷却状态。</p>}
    </section>;
}
