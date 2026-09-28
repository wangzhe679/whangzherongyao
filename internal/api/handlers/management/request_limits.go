package management

import (
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/gin-gonic/gin"
)

// Pending requests reserve capacity, preventing concurrent completions from
// overshooting the success limit. Failed requests release their reservation.
type successLimiter struct {
	mu                        sync.Mutex
	limit, successes, pending int64
	start                     time.Time
	now                       func() time.Time
}

func (l *successLimiter) rollWindow() {
	now := l.now()
	if l.start.IsZero() || now.Sub(l.start) >= time.Minute {
		l.start = now
		l.successes = 0
	}
}

func (l *successLimiter) begin() (func(bool), bool) {
	l.mu.Lock()
	l.rollWindow()
	if l.limit > 0 && l.successes+l.pending >= l.limit {
		l.mu.Unlock()
		return nil, false
	}
	l.pending++
	l.mu.Unlock()
	var once sync.Once
	return func(success bool) {
		once.Do(func() {
			l.mu.Lock()
			defer l.mu.Unlock()
			l.rollWindow()
			l.pending--
			if success {
				l.successes++
			}
		})
	}, true
}

func (l *successLimiter) setLimit(limit int64) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.rollWindow()
	l.limit = limit
}

func (l *successLimiter) snapshot() gin.H {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.rollWindow()
	return gin.H{"limit_per_minute": l.limit, "success_count": l.successes, "pending_count": l.pending, "window_start": l.start.UnixMilli()}
}

// RequestSuccessLimit applies to inference POSTs, not model listings or token
// counting. It is installed on both Claude/OpenAI and Gemini protocol groups.
func RequestSuccessLimit(c *gin.Context) {
	if c.Request.Method != http.MethodPost || strings.HasSuffix(c.Request.URL.Path, "/count_tokens") || strings.HasSuffix(c.Request.URL.Path, ":countTokens") {
		c.Next()
		return
	}
	finish, allowed := requestLimits.begin()
	if !allowed {
		c.AbortWithStatusJSON(http.StatusTooManyRequests, gin.H{"error": "per-minute success capacity exhausted"})
		return
	}
	success := false
	defer func() { finish(success) }()
	c.Next()
	status := c.Writer.Status()
	success = status >= 200 && status < 300 && !c.IsAborted() && len(c.Errors) == 0 && c.Request.Context().Err() == nil && !c.GetBool("CPA_REQUEST_FAILED")
}
