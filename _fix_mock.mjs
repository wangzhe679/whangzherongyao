import { readFileSync, writeFileSync } from 'fs';
let s = readFileSync('dev-mock.ts', 'utf8');

// 1. Add supports_thinking to claude-opus-4-6-thinking entries
s = s.replace(
  /display_name:'Claude Opus 4\.6 Thinking' \}/g,
  "display_name:'Claude Opus 4.6 Thinking', supports_thinking:true }"
);

// 2. Bob (acc-002): add gemini-4-argon lock to live_limited_models
s = s.replace(
  "message:'Token bucket exhausted',\n      },\n    },\n  },\n  {\n    id: 'acc-003'",
  "message:'Token bucket exhausted',\n      },\n      'gemini-4-argon': {\n        model:'gemini-4-argon', status:503,\n        reason:'Service temporarily unavailable',\n        until: now + 45*60_000, detected_at: now - 15*60_000,\n        message:'Backend overloaded',\n      },\n    },\n  },\n  {\n    id: 'acc-003'"
);

// 3. Charlie (acc-003): replace empty live_limited_models with opus lock
s = s.replace(
  "live_limited_models: {},\n  },\n  {\n    id: 'acc-004'",
  "live_limited_models: {\n      'claude-opus-4-6-thinking': {\n        model:'claude-opus-4-6-thinking', status:429,\n        reason:'Rate limited - thinking token budget exceeded',\n        until: now + 5*hour, detected_at: now - 1*hour,\n        message:'Thinking token budget exhausted',\n      },\n    },\n  },\n  {\n    id: 'acc-004'"
);

// 4. Charlie: add 3 more models (currently only has sonnet+opus)
s = s.replace(
  "{ name:'claude-opus-4-6-thinking', percentage:15, reset_time:'6h 50m', display_name:'Claude Opus 4.6 Thinking', supports_thinking:true },\n      ],\n      last_updated: now - 3600_000, subscription_tier: 'g1-pro-tier',",
  "{ name:'claude-opus-4-6-thinking', percentage:15, reset_time:'6h 50m', display_name:'Claude Opus 4.6 Thinking', supports_thinking:true },\n        { name:'gemini-3.8-flash-tiered', percentage:40, reset_time:'3h 20m', display_name:'Gemini 3.8 Flash' },\n        { name:'gemini-4-argon', percentage:62, reset_time:'5h 10m', display_name:'Gemini 4 Argon' },\n        { name:'gemini-3.1-flash-lite', percentage:85, reset_time:'1h 45m', display_name:'Gemini 3.1 Flash Lite' },\n      ],\n      last_updated: now - 3600_000, subscription_tier: 'g1-pro-tier',"
);

// 5. Diana: add gemini-3.1-flash-lite
s = s.replace(
  "{ name:'gemini-3.8-flash-tiered', percentage:100, reset_time:'0m', display_name:'Gemini 3.8 Flash' },\n      ],\n      last_updated: now - 86400_000, subscription_tier: 'free',",
  "{ name:'gemini-3.8-flash-tiered', percentage:100, reset_time:'0m', display_name:'Gemini 3.8 Flash' },\n        { name:'gemini-3.1-flash-lite', percentage:100, reset_time:'0m', display_name:'Gemini 3.1 Flash Lite' },\n      ],\n      last_updated: now - 86400_000, subscription_tier: 'free',"
);

// 6. Update pool-status: locked counts
s = s.replace(
  "loaded_count: 5, available_count: 3, locked_account_count: 1, locked_model_count: 1,",
  "loaded_count: 5, available_count: 3, locked_account_count: 2, locked_model_count: 3,"
);

// 7. Update pool-status models array
s = s.replace(
  "{ model:'claude-sonnet-4-6', loaded:4, available:3, locked:1, lock_stats:{ locked:1, true_429:1, fake_429:0, short_locks:0, expires_in_days:[0,1,0,0,0,0], over_six_days:0, earliest_until: Math.floor(now/1000)+7200, latest_until: Math.floor(now/1000)+7200 } },\n              { model:'gemini-3.8-flash-tiered', loaded:4, available:4, locked:0 },\n              { model:'gemini-4-argon', loaded:4, available:4, locked:0 },\n              { model:'gemini-3.1-flash-lite', loaded:3, available:3, locked:0 },\n              { model:'claude-opus-4-6-thinking', loaded:2, available:2, locked:0 },",
  "{ model:'claude-sonnet-4-6', loaded:4, available:3, locked:1, lock_stats:{ locked:1, true_429:1, fake_429:0, short_locks:0, expires_in_days:[0,1,0,0,0,0], over_six_days:0, earliest_until: Math.floor(now/1000)+7200, latest_until: Math.floor(now/1000)+7200 } },\n              { model:'claude-opus-4-6-thinking', loaded:4, available:3, locked:1, lock_stats:{ locked:1, true_429:1, fake_429:0, short_locks:0, expires_in_days:[0,0,1,0,0,0], over_six_days:0, earliest_until: Math.floor(now/1000)+18000, latest_until: Math.floor(now/1000)+18000 } },\n              { model:'gemini-3.8-flash-tiered', loaded:5, available:5, locked:0 },\n              { model:'gemini-4-argon', loaded:4, available:3, locked:1, lock_stats:{ locked:1, true_429:0, fake_429:1, short_locks:0, expires_in_days:[1,0,0,0,0,0], over_six_days:0, earliest_until: Math.floor(now/1000)+2700, latest_until: Math.floor(now/1000)+2700 } },\n              { model:'gemini-3.1-flash-lite', loaded:3, available:3, locked:0 },"
);

// 8. Update locked_accounts: add Bob's 2nd lock + Charlie
s = s.replace(
  "{ account_id:'acc-002', email:'bob.test@outlook.com', loaded:true, quotas:{}, locks:{ 'claude-sonnet-4-6':{ model:'claude-sonnet-4-6', until: Math.floor(now/1000)+7200, detected_at: Math.floor(now/1000)-1800, status:429, lock_type:'exact', transient_count:0, reason:'Rate limited', message:'Token bucket exhausted' } } },\n            ],",
  "{ account_id:'acc-002', email:'bob.test@outlook.com', loaded:true, quotas:{}, locks:{ 'claude-sonnet-4-6':{ model:'claude-sonnet-4-6', until: Math.floor(now/1000)+7200, detected_at: Math.floor(now/1000)-1800, status:429, lock_type:'exact', transient_count:0, reason:'Rate limited', message:'Token bucket exhausted' }, 'gemini-4-argon':{ model:'gemini-4-argon', until: Math.floor(now/1000)+2700, detected_at: Math.floor(now/1000)-900, status:503, lock_type:'transient', transient_count:3, reason:'Service unavailable', message:'Backend overloaded' } } },\n              { account_id:'acc-003', email:'charlie.dev@gmail.com', loaded:true, quotas:{}, locks:{ 'claude-opus-4-6-thinking':{ model:'claude-opus-4-6-thinking', until: Math.floor(now/1000)+18000, detected_at: Math.floor(now/1000)-3600, status:429, lock_type:'exact', transient_count:0, reason:'Rate limited - thinking budget', message:'Thinking token budget exhausted' } } },\n            ],"
);

writeFileSync('dev-mock.ts', s);
console.log('All patches applied successfully');
