package management

import (
	"encoding/json"
	"math"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"strings"
	"sync"
	"time"

	"github.com/gin-gonic/gin"
	log "github.com/sirupsen/logrus"
)

var independentLockModels = []string{
	"claude-opus-4-6",
	"claude-opus-4-6-thinking",
	"gemini-3.1-flash-lite",
	"gemini-3.5-flash-lite",
	"gemini-3.6-flash-high",
	"gemini-3.7-flash-high",
	"gemini-3.8-flash-high",
	"gemini-3-flash",
	"gemini-3.1-flash-image",
	"gemini-3.1-pro-low",
}

var requestLimits = successLimiter{limit: 1000, now: time.Now}
var forbiddenFilesMu sync.Mutex

func (h *Handler) GetDashboardStats(c *gin.Context) {
	if h == nil || h.authManager == nil {
		c.JSON(http.StatusServiceUnavailable, gin.H{"error": "auth manager unavailable"})
		return
	}

	now := time.Now()
	auths := h.authManager.List()
	authDir := ""
	if h.cfg != nil {
		authDir = h.cfg.AuthDir
	}

	totalAccounts := len(auths)
	availableAccounts := 0
	disabledAccounts := 0
	forbiddenAccounts := 0

	modelAvailable := make(map[string]int)
	modelTotal := make(map[string]int)
	modelPrecise := make(map[string]int)
	modelShort := make(map[string]int)
	modelTomorrowRecover := make(map[string]int)
	for _, m := range independentLockModels {
		modelAvailable[m] = 0
		modelTotal[m] = 0
		modelPrecise[m] = 0
		modelShort[m] = 0
		modelTomorrowRecover[m] = 0
	}

	tomorrowStart := time.Date(now.Year(), now.Month(), now.Day()+1, 0, 0, 0, 0, now.Location())
	tomorrowEnd := tomorrowStart.Add(24 * time.Hour)

	var lockedAccountsList []gin.H

	for _, auth := range auths {
		if auth == nil {
			continue
		}
		if auth.Disabled {
			disabledAccounts++
			continue
		}

		is403 := false
		if auth.LastError != nil {
			if auth.LastError.HTTPStatus == 403 {
				is403 = true
				forbiddenAccounts++
				if authDir != "" && auth.FileName != "" {
					move403AuthFile(authDir, auth.FileName)
				}
			}
		}

		if !is403 {
			availableAccounts++
		}

		for _, modelName := range independentLockModels {
			modelTotal[modelName]++
			lockedPrecise := false
			if auth.ModelStates != nil {
				if state, ok := auth.ModelStates[modelName]; ok && state != nil {
					if state.Unavailable && !state.NextRetryAfter.IsZero() && state.NextRetryAfter.After(now) {
						reason := strings.TrimSpace(state.Quota.Reason)
						if reason == "service_unavailable" {
							modelShort[modelName]++
						} else {
							lockedPrecise = true
							modelPrecise[modelName]++
							if state.NextRetryAfter.After(tomorrowStart) && state.NextRetryAfter.Before(tomorrowEnd) {
								modelTomorrowRecover[modelName]++
							}
						}
					}
				}
			}
			if !is403 && !lockedPrecise {
				modelAvailable[modelName]++
			}
		}

		if auth.ModelStates != nil {
			hasLock := false
			var mlocks []gin.H
			for _, mn := range independentLockModels {
				st, ok := auth.ModelStates[mn]
				if ok && st != nil && !st.NextRetryAfter.IsZero() && st.NextRetryAfter.After(now) {
					hasLock = true
					mlocks = append(mlocks, gin.H{
						"model":             mn,
						"locked":            true,
						"reason":            st.Quota.Reason,
						"expires":           st.NextRetryAfter,
						"remaining_seconds": int(st.NextRetryAfter.Sub(now).Seconds()),
					})
				} else {
					mlocks = append(mlocks, gin.H{"model": mn, "locked": false})
				}
			}
			if hasLock {
				lockedAccountsList = append(lockedAccountsList, gin.H{"name": auth.FileName, "models": mlocks})
			}
		}
	}

	forbidden403Count := count403Folder(authDir)

	modelStats := make([]gin.H, 0, len(independentLockModels))
	for _, modelName := range independentLockModels {
		total := modelTotal[modelName]
		avail := modelAvailable[modelName]
		pct := 0
		if total > 0 {
			pct = avail * 100 / total
		}
		modelStats = append(modelStats, gin.H{
			"model":            modelName,
			"total":            total,
			"available":        avail,
			"precise_locked":   modelPrecise[modelName],
			"short_locked":     modelShort[modelName],
			"available_pct":    pct,
			"tomorrow_recover": modelTomorrowRecover[modelName],
		})
	}

	var memStats runtime.MemStats
	runtime.ReadMemStats(&memStats)

	c.JSON(http.StatusOK, gin.H{
		"observed_at":         now,
		"total_accounts":      totalAccounts,
		"available_accounts":  availableAccounts,
		"disabled_accounts":   disabledAccounts,
		"forbidden_accounts":  forbiddenAccounts,
		"forbidden_403_count": forbidden403Count,
		"model_stats":         modelStats,
		"memory": gin.H{
			"alloc_mb":      memStats.Alloc / 1024 / 1024,
			"sys_mb":        memStats.Sys / 1024 / 1024,
			"heap_inuse_mb": memStats.HeapInuse / 1024 / 1024,
			"heap_objects":  memStats.HeapObjects,
			"goroutines":    runtime.NumGoroutine(),
		},
		"locked_accounts": lockedAccountsList,
		"rate_limit":      requestLimits.snapshot(),
	})
}

func move403AuthFile(authDir, fileName string) {
	if authDir == "" || fileName == "" || filepath.Base(fileName) != fileName {
		return
	}
	forbiddenFilesMu.Lock()
	defer forbiddenFilesMu.Unlock()
	src := filepath.Join(authDir, fileName)
	if _, err := os.Stat(src); err != nil {
		return
	}
	dir403 := filepath.Join(filepath.Dir(authDir), "auth_403")
	os.MkdirAll(dir403, 0755)
	dst := filepath.Join(dir403, fileName)
	data, err := os.ReadFile(src)
	if err != nil {
		return
	}
	if err := os.WriteFile(dst, data, 0600); err != nil {
		return
	}
	os.Remove(src)
}

func count403Folder(authDir string) int {
	if authDir == "" {
		return 0
	}
	dir403 := filepath.Join(filepath.Dir(authDir), "auth_403")
	entries, err := os.ReadDir(dir403)
	if err != nil {
		return 0
	}
	n := 0
	for _, e := range entries {
		if !e.IsDir() && strings.HasSuffix(e.Name(), ".json") {
			n++
		}
	}
	return n
}

func (h *Handler) GetRequestRateLimit(c *gin.Context) {
	c.JSON(http.StatusOK, requestLimits.snapshot())
}

func (h *Handler) SetRequestRateLimit(c *gin.Context) {
	var req struct {
		Limit int64 `json:"limit"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request body"})
		return
	}
	if req.Limit < 0 {
		req.Limit = 0
	}
	requestLimits.setLimit(req.Limit)
	c.JSON(http.StatusOK, gin.H{"status": "ok", "limit_per_minute": req.Limit})
}

func (h *Handler) GetMemoryStats(c *gin.Context) {
	var memStats runtime.MemStats
	runtime.ReadMemStats(&memStats)
	c.JSON(http.StatusOK, gin.H{
		"alloc_mb":       memStats.Alloc / 1024 / 1024,
		"total_alloc_mb": memStats.TotalAlloc / 1024 / 1024,
		"sys_mb":         memStats.Sys / 1024 / 1024,
		"heap_alloc_mb":  memStats.HeapAlloc / 1024 / 1024,
		"heap_inuse_mb":  memStats.HeapInuse / 1024 / 1024,
		"heap_objects":   memStats.HeapObjects,
		"goroutines":     runtime.NumGoroutine(),
		"gc_cycles":      memStats.NumGC,
	})
}

func (h *Handler) DeleteForbiddenAccounts(c *gin.Context) {
	forbiddenFilesMu.Lock()
	defer forbiddenFilesMu.Unlock()
	authDir := ""
	if h.cfg != nil {
		authDir = h.cfg.AuthDir
	}
	if authDir == "" {
		c.JSON(http.StatusOK, gin.H{"status": "ok", "removed": 0})
		return
	}
	dir403 := filepath.Join(filepath.Dir(authDir), "auth_403")
	entries, err := os.ReadDir(dir403)
	if err != nil {
		if os.IsNotExist(err) {
			c.JSON(http.StatusOK, gin.H{"status": "ok", "count": 0, "files": []any{}})
		} else {
			c.JSON(http.StatusInternalServerError, gin.H{"error": "cannot read forbidden account directory"})
		}
		return
	}
	var files []map[string]interface{}
	type exportedFile struct {
		path string
		info os.FileInfo
	}
	var exported []exportedFile
	for _, e := range entries {
		if !e.Type().IsRegular() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		full := filepath.Join(dir403, e.Name())
		info, statErr := os.Lstat(full)
		if statErr != nil || !info.Mode().IsRegular() {
			continue
		}
		data, readErr := os.ReadFile(full)
		if readErr != nil {
			continue
		}
		var parsed map[string]interface{}
		if jsonErr := json.Unmarshal(data, &parsed); jsonErr != nil || parsed == nil {
			continue
		}
		parsed["_filename"] = e.Name()
		files = append(files, parsed)
		exported = append(exported, exportedFile{full, info})
	}
	payload, err := json.Marshal(gin.H{
		"status": "ok",
		"count":  len(files),
		"files":  files,
	})
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "cannot encode account export"})
		return
	}
	c.Header("Content-Type", "application/json")
	c.Header("Cache-Control", "no-store")
	c.Status(http.StatusOK)
	// Keep credentials on disk if writing the export fails. Delete only the
	// exact unchanged regular files included in this response.
	if n, errWrite := c.Writer.Write(payload); errWrite != nil || n != len(payload) {
		return
	}
	for _, file := range exported {
		info, errStat := os.Lstat(file.path)
		if errStat != nil || !os.SameFile(file.info, info) || !info.Mode().IsRegular() || info.Size() != file.info.Size() || !info.ModTime().Equal(file.info.ModTime()) {
			continue
		}
		if errRemove := os.Remove(file.path); errRemove != nil {
			log.Warn("failed to remove an exported forbidden account file")
		}
	}
}

func (h *Handler) GetMemoryLimit(c *gin.Context) {
	limitMB := effectiveMemoryLimit() / 1024 / 1024
	var memStats runtime.MemStats
	runtime.ReadMemStats(&memStats)
	c.JSON(http.StatusOK, gin.H{
		"limit_mb":   limitMB,
		"current_mb": memStats.Alloc / 1024 / 1024,
		"sys_mb":     memStats.Sys / 1024 / 1024,
		"soft_limit": true,
	})
}

func (h *Handler) SetMemoryLimit(c *gin.Context) {
	var req struct {
		LimitGB float64 `json:"limit_gb"`
	}
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": "invalid request"})
		return
	}
	if req.LimitGB < 0 {
		req.LimitGB = 0
	}
	if req.LimitGB >= float64(math.MaxInt64)/(1024*1024*1024) {
		c.JSON(http.StatusBadRequest, gin.H{"error": "memory limit is too large"})
		return
	}
	limit := int64(req.LimitGB * 1024 * 1024 * 1024)
	if limit == 0 {
		limit = math.MaxInt64
	}
	debug.SetMemoryLimit(limit)
	c.JSON(http.StatusOK, gin.H{"status": "ok", "limit_gb": req.LimitGB, "soft_limit": true})
}

func effectiveMemoryLimit() int64 {
	limit := debug.SetMemoryLimit(-1)
	if limit == math.MaxInt64 {
		return 0
	}
	return limit
}
