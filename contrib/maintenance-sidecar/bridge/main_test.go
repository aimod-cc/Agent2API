package main

import (
	"bufio"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
	"time"
)

func TestUsageBridgeAndUnchangedModelFrames(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("The reference adapter uses Linux process signals and a fixture shell child")
	}
	requests := make(chan map[string]string, 4)
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" || r.Header.Get("Authorization") != "Bearer fixture-only" {
			t.Error("Unexpected method or missing bridge authentication")
			w.WriteHeader(403)
			return
		}
		var params map[string]string
		if err := json.NewDecoder(r.Body).Decode(&params); err != nil {
			t.Error(err)
		}
		requests <- params
		w.Header().Set("Content-Type", "application/json")
		if params["account"] == "error" {
			w.Write([]byte(`{"error":{"code":"upstream","message":"fixture rejected"}}`))
			return
		}
		w.Write([]byte(`{"result":{"totalLeft":123}}`))
	}))
	defer upstream.Close()
	dir := t.TempDir()
	bridge := filepath.Join(dir, "bridge")
	if data, err := exec.Command("go", "build", "-o", bridge, ".").CombinedOutput(); err != nil {
		t.Fatalf("build: %v %s", err, data)
	}
	// The original plugin is replaced only inside this temporary fixture directory.
	if err := os.WriteFile(filepath.Join(dir, "agent2api-providers.official"), []byte("#!/bin/sh\nexec cat\n"), 0700); err != nil {
		t.Fatal(err)
	}
	configPath := filepath.Join(dir, "fixture.json")
	data, _ := json.Marshal(config{URL: upstream.URL, Token: "fixture-only", Timeout: 10})
	if err := os.WriteFile(configPath, data, 0600); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(bridge)
	cmd.Env = append(os.Environ(), "A2A_MAINTENANCE_BRIDGE_CONFIG="+configPath)
	stdin, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { stdin.Close(); cmd.Process.Kill(); cmd.Wait() }()
	scanner := bufio.NewScanner(stdout)
	lines := make(chan string, 8)
	go func() {
		for scanner.Scan() {
			lines <- scanner.Text()
		}
		close(lines)
	}()
	read := func() string {
		select {
		case line, ok := <-lines:
			if !ok {
				t.Fatal("Bridge exited unexpectedly")
			}
			return line
		case <-time.After(15 * time.Second):
			t.Fatal("Timed out waiting for fixture response")
			return ""
		}
	}
	// Model calls, unrelated providers and plugin callbacks are byte-preserved.
	for _, frame := range []string{
		`{"id":1,"method":"chat.completions","params":{"provider":"loomy","account":"fixture"}}`,
		`{"id":2,"method":"usage.query","params":{"provider":"other","account":"fixture"}}`,
		`{"id":3,"result":{"secret":"synthetic-callback"}}`,
	} {
		stdin.Write([]byte(frame + "\n"))
		if got := read(); got != frame {
			t.Fatalf("frame changed: %s", got)
		}
	}
	for _, provider := range []string{"loomy", "raccoon"} {
		frame := `{"id":4,"method":"usage.query","params":{"provider":"` + provider + `","account":"fixture"}}`
		stdin.Write([]byte(frame + "\n"))
		var response struct {
			ID     int
			Result struct{ TotalLeft int }
			Error  any
		}
		if err := json.Unmarshal([]byte(read()), &response); err != nil {
			t.Fatal(err)
		}
		if response.ID != 4 || response.Result.TotalLeft != 123 || response.Error != nil {
			t.Fatalf("bad response: %+v", response)
		}
		select {
		case params := <-requests:
			if params["provider"] != provider {
				t.Fatal("provider not forwarded")
			}
		case <-time.After(time.Second):
			t.Fatal("bridge not called")
		}
	}
	stdin.Write([]byte(`{"id":5,"method":"usage.query","params":{"provider":"loomy","account":"error"}}` + "\n"))
	var failure struct {
		ID     int
		Error  struct{ Code, Message string }
		Result any
	}
	if err := json.Unmarshal([]byte(read()), &failure); err != nil {
		t.Fatal(err)
	}
	if failure.ID != 5 || failure.Error.Code != "upstream" || failure.Result != nil {
		t.Fatalf("upstream error not preserved: %+v", failure)
	}
}
