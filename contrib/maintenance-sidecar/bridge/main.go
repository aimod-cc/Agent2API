// Local maintenance bridge. Passes all original plugin frames unchanged except
// usage.query for the two explicitly supported providers. No model interception.
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"sync"
	"syscall"
	"time"
)

type config struct {
	URL     string `json:"url"`
	Token   string `json:"token"`
	Timeout int    `json:"timeout_seconds"`
}
type frame struct {
	ID     json.RawMessage `json:"id"`
	Method string          `json:"method"`
	Params json.RawMessage `json:"params"`
}
type parameters struct {
	Provider string `json:"provider"`
	Account  string `json:"account"`
}

var output sync.Mutex

func write(b []byte) {
	output.Lock()
	defer output.Unlock()
	os.Stdout.Write(b)
	if len(b) == 0 || b[len(b)-1] != '\n' {
		os.Stdout.Write([]byte{'\n'})
	}
}
func failure(id json.RawMessage, message string) {
	b, _ := json.Marshal(map[string]any{"id": id, "error": map[string]any{"message": message, "code": "maintenance_bridge"}})
	write(b)
}
func query(f frame, c config) {
	req, err := http.NewRequest("POST", c.URL, bytes.NewReader(f.Params))
	if err != nil {
		failure(f.ID, "维护桥请求配置错误")
		return
	}
	req.Header.Set("Authorization", "Bearer "+c.Token)
	req.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: time.Duration(c.Timeout) * time.Second, Transport: &http.Transport{Proxy: nil, DisableKeepAlives: true}, CheckRedirect: func(req *http.Request, via []*http.Request) error { return http.ErrUseLastResponse }}
	resp, err := client.Do(req)
	if err != nil {
		failure(f.ID, "维护容器连接失败或超时；未回退直连")
		return
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	if err != nil {
		failure(f.ID, "维护结果读取失败")
		return
	}
	var result map[string]json.RawMessage
	if json.Unmarshal(body, &result) != nil {
		failure(f.ID, "维护容器返回了无效结果")
		return
	}
	if e, ok := result["error"]; ok {
		b, _ := json.Marshal(map[string]any{"id": f.ID, "error": e})
		write(b)
		return
	}
	if resp.StatusCode != 200 {
		failure(f.ID, fmt.Sprintf("维护容器返回 HTTP %d", resp.StatusCode))
		return
	}
	u, ok := result["result"]
	if !ok {
		failure(f.ID, "维护结果缺少余额字段")
		return
	}
	b, _ := json.Marshal(map[string]any{"id": f.ID, "result": u})
	write(b)
}
func main() {
	exe, err := os.Executable()
	if err != nil {
		os.Exit(1)
	}
	cfgPath := "/data/plugins/maintenance-bridge.private.json"
	if x := os.Getenv("A2A_MAINTENANCE_BRIDGE_CONFIG"); x != "" {
		cfgPath = x
	}
	var cfg config
	b, err := os.ReadFile(cfgPath)
	if err != nil || json.Unmarshal(b, &cfg) != nil || cfg.Token == "" || cfg.URL == "" {
		fmt.Fprintln(os.Stderr, "maintenance bridge config unavailable")
		os.Exit(1)
	}
	if cfg.Timeout < 10 || cfg.Timeout > 110 {
		cfg.Timeout = 70
	}
	child := exec.Command(filepath.Join(filepath.Dir(exe), "agent2api-providers.official"))
	child.Env = os.Environ()
	child.Stderr = os.Stderr
	stdin, err := child.StdinPipe()
	if err != nil {
		os.Exit(1)
	}
	stdout, err := child.StdoutPipe()
	if err != nil {
		os.Exit(1)
	}
	if child.Start() != nil {
		os.Exit(1)
	}
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGTERM, syscall.SIGINT)
	go func() {
		<-stop
		child.Process.Signal(syscall.SIGTERM)
		time.Sleep(2 * time.Second)
		child.Process.Kill()
		os.Exit(0)
	}()
	go func() {
		reader := bufio.NewReader(stdout)
		for {
			line, e := reader.ReadBytes('\n')
			if len(line) > 0 {
				write(line)
			}
			if e != nil {
				break
			}
		}
		child.Wait()
		os.Exit(0)
	}()
	reader := bufio.NewReader(os.Stdin)
	semaphore := make(chan struct{}, 32)
	for {
		line, e := reader.ReadBytes('\n')
		if len(line) > 0 {
			var f frame
			var p parameters
			intercept := json.Unmarshal(line, &f) == nil && f.Method == "usage.query" && json.Unmarshal(f.Params, &p) == nil && (p.Provider == "raccoon" || p.Provider == "loomy") && p.Account != ""
			if intercept {
				select {
				case semaphore <- struct{}{}:
					go func(f frame) { defer func() { <-semaphore }(); query(f, cfg) }(f)
				default:
					failure(f.ID, "维护余额并发已满，请由维护队列分批提交")
				}
			} else {
				if _, err := stdin.Write(line); err != nil {
					break
				}
			}
		}
		if e != nil {
			break
		}
	}
	stdin.Close()
	child.Process.Signal(syscall.SIGTERM)
	child.Wait()
}
