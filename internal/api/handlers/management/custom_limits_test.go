package management

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime/debug"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/tidwall/gjson"
)

func TestCustomSuccessLimitConcurrentAndRollover(t *testing.T) {
	now := time.Unix(1000, 0)
	l := successLimiter{limit: 30, now: func() time.Time { return now }}
	var accepted atomic.Int64
	var wg sync.WaitGroup
	for i := 0; i < 1000; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if finish, ok := l.begin(); ok {
				accepted.Add(1)
				finish(true)
			}
		}()
	}
	wg.Wait()
	if accepted.Load() != 30 {
		t.Fatalf("accepted=%d", accepted.Load())
	}
	now = now.Add(time.Minute)
	finish, ok := l.begin()
	if !ok {
		t.Fatal("minute window did not reset")
	}
	finish(false)
	if l.snapshot()["success_count"] != int64(0) {
		t.Fatal("failed request counted")
	}
	finish, ok = l.begin()
	if !ok {
		t.Fatal("failure did not release capacity")
	}
	now = now.Add(time.Minute)
	finish(true)
	finish(true)
	if l.snapshot()["success_count"] != int64(1) || l.snapshot()["pending_count"] != int64(0) {
		t.Fatal("completion rollover or duplicate finish broken")
	}
}

func TestCustomSuccessLimitProtocolsAndErrors(t *testing.T) {
	requestLimits.setLimit(1000)
	start := requestLimits.snapshot()["success_count"].(int64)
	t.Cleanup(func() { requestLimits.setLimit(1000) })
	r := gin.New()
	for _, prefix := range []string{"/v1", "/v1beta"} {
		g := r.Group(prefix, RequestSuccessLimit)
		g.POST("/ok", func(c *gin.Context) { c.Status(200) })
		g.POST("/bad", func(c *gin.Context) { c.Status(400) })
		g.POST("/forbidden", func(c *gin.Context) { c.Status(403) })
		g.POST("/stream-error", func(c *gin.Context) { c.Set("CPA_REQUEST_FAILED", true); c.Status(200) })
		g.GET("/models", func(c *gin.Context) { c.Status(200) })
	}
	for _, prefix := range []string{"/v1", "/v1beta"} {
		for _, path := range []string{"/ok", "/bad", "/forbidden", "/stream-error", "/models"} {
			method := "POST"
			if path == "/models" {
				method = "GET"
			}
			r.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(method, prefix+path, nil))
		}
	}
	count := requestLimits.snapshot()["success_count"].(int64)
	if count-start != 2 {
		t.Fatalf("successful calls=%d want 2", count-start)
	}
	requestLimits.setLimit(count)
	w := httptest.NewRecorder()
	r.ServeHTTP(w, httptest.NewRequest("POST", "/v1beta/ok", nil))
	if w.Code != 429 {
		t.Fatalf("Gemini limit status=%d", w.Code)
	}
}

func TestCustomMemoryLimitAppliesToRuntime(t *testing.T) {
	previous := debug.SetMemoryLimit(-1)
	defer debug.SetMemoryLimit(previous)
	h := &Handler{}
	r := gin.New()
	r.PUT("/memory", h.SetMemoryLimit)
	r.GET("/memory", h.GetMemoryLimit)
	w := httptest.NewRecorder()
	r.ServeHTTP(w, httptest.NewRequest("PUT", "/memory", strings.NewReader(`{"limit_gb":2}`)))
	if w.Code != 200 || debug.SetMemoryLimit(-1) != 2<<30 {
		t.Fatalf("limit not applied: %s", w.Body.String())
	}
	w = httptest.NewRecorder()
	r.ServeHTTP(w, httptest.NewRequest("GET", "/memory", nil))
	if gjson.Get(w.Body.String(), "limit_mb").Int() != 2048 {
		t.Fatal(w.Body.String())
	}
	w = httptest.NewRecorder()
	r.ServeHTTP(w, httptest.NewRequest("PUT", "/memory", strings.NewReader(`{"limit_gb":1e30}`)))
	if w.Code != 400 || debug.SetMemoryLimit(-1) != 2<<30 {
		t.Fatal("overflow accepted")
	}
}

type failedExportWriter struct{ *httptest.ResponseRecorder }

func (w failedExportWriter) Write([]byte) (int, error) { return 0, errors.New("client disconnected") }

func TestCustomForbiddenExportDeletesOnlyExportedFiles(t *testing.T) {
	for _, failWrite := range []bool{false, true} {
		t.Run(map[bool]string{false: "success", true: "write-error"}[failWrite], func(t *testing.T) {
			base := t.TempDir()
			authDir := filepath.Join(base, "auths")
			forbidden := filepath.Join(base, "auth_403")
			for _, dir := range []string{authDir, forbidden} {
				if err := os.Mkdir(dir, 0700); err != nil {
					t.Fatal(err)
				}
			}
			active := filepath.Join(authDir, "active.json")
			good := filepath.Join(forbidden, "blocked.json")
			bad := filepath.Join(forbidden, "invalid.json")
			for path, content := range map[string]string{active: `{"locks":"retain"}`, good: `{"refresh_token":"test-rt"}`, bad: `not-json`} {
				if err := os.WriteFile(path, []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			}
			h := &Handler{cfg: &config.Config{AuthDir: authDir}}
			r := gin.New()
			r.DELETE("/forbidden", h.DeleteForbiddenAccounts)
			w := httptest.NewRecorder()
			var writer http.ResponseWriter = w
			if failWrite {
				writer = failedExportWriter{w}
			}
			r.ServeHTTP(writer, httptest.NewRequest("DELETE", "/forbidden", nil))
			_, err := os.Stat(good)
			if failWrite && err != nil {
				t.Fatal("export failure deleted credentials")
			}
			if !failWrite {
				if !os.IsNotExist(err) {
					t.Fatal("exported file was not deleted")
				}
				if gjson.Get(w.Body.String(), "files.0.refresh_token").String() != "test-rt" {
					t.Fatal("missing exported RT")
				}
			}
			for _, path := range []string{active, bad} {
				if _, err := os.Stat(path); err != nil {
					t.Fatalf("unexpected deletion: %s", path)
				}
			}
		})
	}
}
