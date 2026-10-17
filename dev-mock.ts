// dev-mock.ts -- Vite middleware plugin: mock API for UI preview without Rust backend
import type { Plugin, Connect } from 'vite';
import type { IncomingMessage, ServerResponse } from 'http';

const now = Date.now();
const hour = 3600_000;

const MOCK_ACCOUNTS = [
  {
    id: 'acc-001', email: 'alice@gmail.com', name: 'Alice', priority: 1,
    custom_label: 'Main PRO', disabled: false, proxy_disabled: false,
    created_at: now - 30*24*hour, last_used: now - 5*60_000,
    token: { access_token:'at-001', refresh_token:'rt-001', expires_in:3600, expiry_timestamp: now+hour, token_type:'Bearer', email:'alice@gmail.com' },
    quota: {
      models: [
        { name:'claude-sonnet-4-6', percentage:72, reset_time:'3h 12m', display_name:'Claude Sonnet 4', supports_thinking:true },
        { name:'claude-opus-4-6-thinking', percentage:45, reset_time:'1h 30m', display_name:'Claude Opus 4.6 Thinking', supports_thinking:true },
        { name:'gemini-3.8-flash-tiered', percentage:88, reset_time:'5h 0m', display_name:'Gemini 3.8 Flash' },
        { name:'gemini-4-argon', percentage:95, reset_time:'4h 22m', display_name:'Gemini 4 Argon' },
        { name:'gemini-3.1-flash-lite', percentage:60, reset_time:'2h 15m', display_name:'Gemini 3.1 Flash Lite' },
      ],
      last_updated: now - 60_000, subscription_tier: 'g1-pro-tier',
      quota_groups: [
        { display_name:'Gemini Models', buckets:[
          { bucket_id:'gem-w', window:'weekly', remaining_fraction:0.88, reset_time:'5d 3h', cycle_tokens:125000 },
          { bucket_id:'gem-5', window:'5h', remaining_fraction:0.72, reset_time:'3h 12m', cycle_tokens:45000 },
        ]},
        { display_name:'Claude models', buckets:[
          { bucket_id:'cg-w', window:'weekly', remaining_fraction:0.45, reset_time:'2d 8h', cycle_tokens:280000 },
        ]},
      ],
    },
    live_limited_models: {},
  },
  {
    id: 'acc-002', email: 'bob.test@outlook.com', name: 'Bob', priority: 2,
    custom_label: 'Ultra Account', disabled: false, proxy_disabled: false,
    created_at: now - 15*24*hour, last_used: now - 2*hour,
    token: { access_token:'at-002', refresh_token:'rt-002', expires_in:3600, expiry_timestamp: now+hour, token_type:'Bearer', email:'bob.test@outlook.com' },
    quota: {
      models: [
        { name:'claude-sonnet-4-6', percentage:30, reset_time:'6h 45m', display_name:'Claude Sonnet 4', supports_thinking:true },
        { name:'gemini-3.8-flash-tiered', percentage:55, reset_time:'2h 30m', display_name:'Gemini 3.8 Flash' },
        { name:'gemini-4-argon', percentage:100, reset_time:'0m', display_name:'Gemini 4 Argon' },
        { name:'gemini-3.1-flash-lite', percentage:20, reset_time:'4h 50m', display_name:'Gemini 3.1 Flash Lite' },
      ],
      last_updated: now - 120_000, subscription_tier: 'GDP_HELIUM',
    },
    live_limited_models: {
      'claude-sonnet-4-6': {
        model:'claude-sonnet-4-6', status:429,
        reason:'Rate limited - too many requests',
        until: now + 2*hour, detected_at: now - 30*60_000,
        message:'Token bucket exhausted',
      },
      'gemini-4-argon': {
        model:'gemini-4-argon', status:503,
        reason:'Service temporarily unavailable',
        until: now + 45*60_000, detected_at: now - 15*60_000,
        message:'Backend overloaded',
      },
    },
  },
  {
    id: 'acc-003', email: 'charlie.dev@gmail.com', name: 'Charlie', priority: 3,
    custom_label: '', disabled: false,
    proxy_disabled: true, proxy_disabled_reason: 'Manual pause', proxy_disabled_at: now - 4*hour,
    created_at: now - 60*24*hour, last_used: now - 12*hour,
    token: { access_token:'at-003', refresh_token:'rt-003', expires_in:3600, expiry_timestamp: now+hour, token_type:'Bearer', email:'charlie.dev@gmail.com' },
    quota: {
      models: [
        { name:'claude-sonnet-4-6', percentage:10, reset_time:'7h 0m', display_name:'Claude Sonnet 4', supports_thinking:true },
        { name:'gemini-3.8-flash-tiered', percentage:15, reset_time:'6h 50m', display_name:'Gemini 3.8 Flash' },
      ],
      last_updated: now - 3600_000, subscription_tier: 'g1-pro-tier',
    },
    live_limited_models: {
      'claude-opus-4-6-thinking': {
        model:'claude-opus-4-6-thinking', status:429,
        reason:'Rate limited - thinking token budget exceeded',
        until: now + 5*hour, detected_at: now - 1*hour,
        message:'Thinking token budget exhausted',
      },
    },
  },
  {
    id: 'acc-004', email: 'diana.free@yahoo.com', name: 'Diana', priority: 4,
    disabled: true, disabled_reason: 'Account suspended by provider', disabled_at: now - 2*24*hour,
    proxy_disabled: false,
    created_at: now - 90*24*hour, last_used: now - 48*hour,
    token: { access_token:'at-004', refresh_token:'rt-004', expires_in:3600, expiry_timestamp: now+hour, token_type:'Bearer', email:'diana.free@yahoo.com' },
    quota: {
      models: [
        { name:'gemini-4-argon', percentage:100, reset_time:'0m', display_name:'Gemini 4 Argon' },
      ],
      last_updated: now - 86400_000, subscription_tier: 'free',
    },
    live_limited_models: {},
  },
  {
    id: 'acc-005', email: 'eve.power@proton.me', name: 'Eve', priority: 5,
    custom_label: 'Backup PRO', disabled: false, proxy_disabled: false,
    created_at: now - 7*24*hour, last_used: now - 30*60_000,
    token: { access_token:'at-005', refresh_token:'rt-005', expires_in:3600, expiry_timestamp: now+hour, token_type:'Bearer', email:'eve.power@proton.me' },
    quota: {
      models: [
        { name:'claude-sonnet-4-6', percentage:92, reset_time:'1h 5m', display_name:'Claude Sonnet 4', supports_thinking:true },
        { name:'claude-opus-4-6-thinking', percentage:80, reset_time:'45m', display_name:'Claude Opus 4.6 Thinking', supports_thinking:true },
        { name:'gemini-3.8-flash-tiered', percentage:65, reset_time:'3h 20m', display_name:'Gemini 3.8 Flash' },
        { name:'gemini-4-argon', percentage:78, reset_time:'2h 10m', display_name:'Gemini 4 Argon' },
        { name:'gemini-3.1-flash-lite', percentage:50, reset_time:'5h 0m', display_name:'Gemini 3.1 Flash Lite' },
      ],
      last_updated: now - 30_000, subscription_tier: 'standard-tier',
      quota_groups: [
        { display_name:'Gemini Models', buckets:[
          { bucket_id:'gem-w', window:'weekly', remaining_fraction:0.65, reset_time:'4d 12h', cycle_tokens:180000 },
          { bucket_id:'gem-5', window:'5h', remaining_fraction:0.78, reset_time:'2h 10m', cycle_tokens:32000 },
        ]},
        { display_name:'Claude models', buckets:[
          { bucket_id:'cg-w', window:'weekly', remaining_fraction:0.92, reset_time:'6d 2h', cycle_tokens:45000 },
        ]},
      ],
    },
    live_limited_models: {},
  },
];

const MOCK_CONFIG: Record<string,any> = {
  language:'zh', theme:'dark', proxy_port:8045, api_key:'mock-dev-key',
  max_successes_per_minute:30, max_concurrent_requests:10, max_memory_gb:4,
  model_mapping:{}, auto_launch:false, telemetry:false,
  auto_refresh:true, refresh_interval:30, auto_sync:false, sync_interval:60,
  auto_check_update:false, accounts_page_size:0, hidden_menu_items:[],
  scheduled_warmup:{ enabled:false, interval_hours:6, cron:'' },
  quota_protection:{ enabled:false, threshold:10 },
  pinned_quota_models:{ models:[] },
  circuit_breaker:{ enabled:false, threshold:5, cooldown_seconds:300 },
  proxy:{ port:8045, host:'127.0.0.1', enable_cors:true, api_key:'mock-dev-key',
    max_successes_per_minute:30, max_concurrent_requests:10, max_memory_gb:4 },
  cloudflared:{ enabled:false, mode:'quick', port:8045, use_http2:false },
};

function json(res: ServerResponse, data: any, status = 200) {
  res.writeHead(status, { 'Content-Type':'application/json' });
  res.end(JSON.stringify(data));
}

export function devMockPlugin(): Plugin {
  return {
    name: 'dev-mock-api',
    configureServer(server) {
      // Insert BEFORE the proxy so /api/ requests are caught here first
      server.middlewares.use((req: IncomingMessage, res: ServerResponse, next: Connect.NextFunction) => {
        const url = req.url || '';
        if (!url.startsWith('/api/')) return next();
        const path = url.split('?')[0];  // strip query string for matching

        // Accounts
        if (path === '/api/accounts' && req.method === 'GET')
          return json(res, { accounts: MOCK_ACCOUNTS });
        if (path === '/api/accounts/current' && req.method === 'GET')
          return json(res, MOCK_ACCOUNTS[0]);
        if (path === '/api/accounts/refresh' && req.method === 'POST')
          return json(res, { total:5, success:5, failed:0, details:[] });

        // Config
        if (path === '/api/config' && req.method === 'GET')
          return json(res, MOCK_CONFIG);
        if (path === '/api/config' && req.method === 'POST')
          return json(res, { ok: true });

        // Proxy status
        if (path === '/api/proxy/status' && req.method === 'GET')
          return json(res, { running:true, port:8045, uptime_seconds:86400, total_requests:1234, active_connections:3, accounts_in_pool:3 });
        if (path === '/api/proxy/pool-status' && req.method === 'GET')
          return json(res, {
            loaded_count: 5, available_count: 3, locked_account_count: 2, locked_model_count: 3,
            models: [
              { model:'claude-sonnet-4-6', loaded:4, available:3, locked:1, lock_stats:{ locked:1, true_429:1, fake_429:0, short_locks:0, expires_in_days:[0,1,0,0,0,0], over_six_days:0, earliest_until: Math.floor(now/1000)+7200, latest_until: Math.floor(now/1000)+7200 } },
              { model:'claude-opus-4-6-thinking', loaded:4, available:3, locked:1, lock_stats:{ locked:1, true_429:1, fake_429:0, short_locks:0, expires_in_days:[0,0,1,0,0,0], over_six_days:0, earliest_until: Math.floor(now/1000)+18000, latest_until: Math.floor(now/1000)+18000 } },
              { model:'gemini-3.8-flash-tiered', loaded:5, available:5, locked:0 },
              { model:'gemini-4-argon', loaded:4, available:3, locked:1, lock_stats:{ locked:1, true_429:0, fake_429:1, short_locks:0, expires_in_days:[1,0,0,0,0,0], over_six_days:0, earliest_until: Math.floor(now/1000)+2700, latest_until: Math.floor(now/1000)+2700 } },
              { model:'gemini-3.1-flash-lite', loaded:3, available:3, locked:0 },
            ],
            locked_accounts: [
              { account_id:'acc-002', email:'bob.test@outlook.com', loaded:true, quotas:{}, locks:{ 'claude-sonnet-4-6':{ model:'claude-sonnet-4-6', until: Math.floor(now/1000)+7200, detected_at: Math.floor(now/1000)-1800, status:429, lock_type:'exact', transient_count:0, reason:'Rate limited', message:'Token bucket exhausted' }, 'gemini-4-argon':{ model:'gemini-4-argon', until: Math.floor(now/1000)+2700, detected_at: Math.floor(now/1000)-900, status:503, lock_type:'transient', transient_count:3, reason:'Service unavailable', message:'Backend overloaded' } } },
              { account_id:'acc-003', email:'charlie.dev@gmail.com', loaded:true, quotas:{}, locks:{ 'claude-opus-4-6-thinking':{ model:'claude-opus-4-6-thinking', until: Math.floor(now/1000)+18000, detected_at: Math.floor(now/1000)-3600, status:429, lock_type:'exact', transient_count:0, reason:'Rate limited - thinking budget', message:'Thinking token budget exhausted' } } },
            ],
            server_time: Math.floor(now/1000), storage_failed: false,
            runtime: { current_gb:1.2, limit_gb:4, in_flight:3, max_concurrent_requests:10, max_successes_per_minute:30, memory_pressure:false },
          });
        if (path === '/api/proxy/export-delete-forbidden' && req.method === 'POST')
          return json(res, { exported:0, deleted:0 });
        if (path === '/api/proxy/stats' && req.method === 'GET')
          return json(res, { total_requests:1234, successful_requests:1180, failed_requests:54, active_connections:3, uptime_seconds:86400 });

        // Stats
        if (url.startsWith('/api/stats/')) return json(res, { data:[], summary:{} });
        // Security
        if (url.startsWith('/api/security/')) return json(res, {});
        // Logs
        if (url.startsWith('/api/logs')) {
          if (url.includes('/count')) return json(res, { count:0 });
          if (url.includes('/disk-size')) return json(res, { size:0 });
          return json(res, { logs:[], total:0 });
        }
        // System
        if (url.startsWith('/api/system/')) {
          if (url.includes('updates/check-status')) return json(res, false);
          if (url.includes('data-dir')) return json(res, { path:'C:\\mock' });
          if (url.includes('autostart')) return json(res, { enabled:false });
          if (url.includes('cache/paths')) return json(res, { paths:[] });
          return json(res, {});
        }
        // User tokens
        if (url.startsWith('/api/user-tokens')) {
          if (url.includes('/summary')) return json(res, { total:0, active:0 });
          return json(res, { tokens:[] });
        }
        // Debug
        if (url.startsWith('/api/debug/')) {
          if (url.includes('/enabled')) return json(res, false);
          return json(res, { logs:[] });
        }
        // Proxy pool
        if (url.startsWith('/api/proxy/pool/')) {
          if (url.includes('/bindings')) return json(res, { bindings:[] });
          return json(res, {});
        }
        // Proxy sub-routes
        if (url.startsWith('/api/proxy/')) {
          if (url.includes('thinking-store/count')) return json(res, { count:0 });
          if (url.includes('preferred-account')) return json(res, null);
          if (url.includes('cloudflared')) return json(res, { installed:false, running:false });
          return json(res, {});
        }
        // OAuth
        if (url.startsWith('/api/accounts/oauth/') || url.startsWith('/api/auth/')) {
          if (url.includes('/clients')) return json(res, { clients:[] });
          return json(res, {});
        }
        // Account sub-routes
        if (url.match(/\/api\/accounts\//)) {
          if (req.method === 'POST' || req.method === 'DELETE') return json(res, { ok:true });
          return json(res, {});
        }

        // Fallback
        return json(res, { mock: true }, 200);
      });
    },
  };
}
