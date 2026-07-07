// e2e smoke harness: simulates the PLC on /dev/shm and drives a running
// plc_bridge over HTTP/WS/Modbus to verify end-to-end behavior — login,
// live data, jog watchdog, stale detection on all consumers.
//
// Usage (no PLC needed):
//
//	python3 -c "open('/dev/shm/plc_data','wb').write(b'\0'*248); open('/dev/shm/plc_cmd','wb').write(b'\0'*168)"
//	go build -o /tmp/plc_bridge ./backend/cmd/plc_bridge
//	(cd /tmp && ./plc_bridge -jog-timeout 500ms -stale-after 500ms &)
//	go run ./backend/e2e_smoke     # prints PASS lines, exits non-zero on failure
//
// It assumes the bridge's built-in default password and plain HTTP on :8443.
package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"sync/atomic"
	"time"
	"unsafe"

	"github.com/gorilla/websocket"

	"codesys_dev/backend/internal/shm"
)

func must(err error, what string) {
	if err != nil {
		log.Fatalf("FAIL %s: %v", what, err)
	}
}

func check(cond bool, what string) {
	if !cond {
		log.Fatalf("FAIL %s", what)
	}
	fmt.Println("PASS", what)
}

func main() {
	// PLC simulator: publish PlcData with an advancing cycle every 10ms.
	dataMap, err := shm.Open(shm.NamePlcData, shm.SizePlcData)
	must(err, "map plc_data")
	seg := (*shm.PlcData)(dataMap.Ptr())
	var simOn atomic.Bool
	simOn.Store(true)
	go func() {
		var cycle uint64
		for {
			if simOn.Load() {
				cycle++
				s := atomic.LoadUint32(&seg.Header.Seq)
				atomic.StoreUint32(&seg.Header.Seq, s+1+(s&1))
				seg.Header.Magic = shm.PlcDataMagic
				seg.Header.Version = shm.PlcDataVersion
				seg.Header.Cycle = cycle
				seg.System.Temperature = 36.5
				atomic.StoreUint32(&seg.Header.Seq, atomic.LoadUint32(&seg.Header.Seq)+1)
			}
			time.Sleep(10 * time.Millisecond)
		}
	}()

	cmdMap, err := shm.Open(shm.NamePlcCommand, shm.SizePlcCommand)
	must(err, "map plc_cmd")
	cmdSeg := (*shm.PlcCommand)(cmdMap.Ptr())

	// Login with the built-in default password, keep the session cookie.
	time.Sleep(300 * time.Millisecond) // let the bridge see fresh data
	body, _ := json.Marshal(map[string]string{"password": "111111"})
	resp, err := http.Post("http://127.0.0.1:8443/api/login", "application/json", bytes.NewReader(body))
	must(err, "login")
	check(resp.StatusCode == 200, "login with default password")
	var cookie string
	for _, c := range resp.Cookies() {
		if c.Name == "plc_session" {
			cookie = c.Name + "=" + c.Value
		}
	}
	check(cookie != "", "session cookie set")

	// WS: live data must not be stale.
	hdr := http.Header{"Cookie": {cookie}}
	ws, _, err := websocket.DefaultDialer.Dial("ws://127.0.0.1:8443/ws", hdr)
	must(err, "ws dial")
	defer ws.Close()

	readData := func() map[string]any {
		for {
			var m map[string]any
			must(ws.ReadJSON(&m), "ws read")
			if m["type"] == "data" {
				return m
			}
		}
	}
	d := readData()
	check(d["stale"] == false, "WS data live (stale=false)")
	check(d["system"].(map[string]any)["temperature"] == 36.5, "WS temperature round-trip")

	// Jog: command sets the bit; the watchdog clears it without keepalives.
	must(ws.WriteJSON(map[string]any{"type": "axis", "axisIndex": 0, "axisFlags": 16, "jogVel": 5}), "send jog")
	time.Sleep(200 * time.Millisecond)
	check(atomic.LoadUint32(&cmdSeg.Machine.Axes[0].ControlFlags)&shm.AxisCtrlJogPos != 0, "jog bit set in plc_cmd")
	time.Sleep(900 * time.Millisecond) // > jog-timeout, no keepalive sent
	check(atomic.LoadUint32(&cmdSeg.Machine.Axes[0].ControlFlags)&shm.AxisCtrlJogPos == 0, "jog bit cleared by watchdog")

	// Modbus: live read succeeds, then the dead PLC turns reads into exception 04.
	mb := func() []byte {
		conn, err := net.Dial("tcp", "127.0.0.1:5020")
		must(err, "modbus dial")
		defer conn.Close()
		req := []byte{0, 1, 0, 0, 0, 6, 1, 0x03, 0, 8, 0, 4} // read 4 regs @ temperature
		_, err = conn.Write(req)
		must(err, "modbus write")
		buf := make([]byte, 256)
		conn.SetReadDeadline(time.Now().Add(2 * time.Second))
		n, err := conn.Read(buf)
		must(err, "modbus read")
		return buf[7:n] // PDU
	}
	pdu := mb()
	check(pdu[0] == 0x03 && pdu[1] == 8, "modbus live read ok")
	temp := binary.BigEndian.Uint64(pdu[2:10])
	check(*(*float64)(unsafe.Pointer(&temp)) == 36.5, "modbus temperature value")

	simOn.Store(false) // PLC "dies" — segment stays readable, cycle frozen
	time.Sleep(900 * time.Millisecond)
	pdu = mb()
	check(pdu[0] == 0x83 && pdu[1] == 0x04, "modbus stale read -> exception 04")
	// Pushes are buffered client-side; drain until the stale flag arrives.
	staleSeen := false
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if readData()["stale"] == true {
			staleSeen = true
			break
		}
	}
	check(staleSeen, "WS flags stale data after PLC death")

	simOn.Store(true) // PLC "recovers"
	time.Sleep(400 * time.Millisecond)
	pdu = mb()
	check(pdu[0] == 0x03, "modbus recovers with PLC")

	fmt.Println("ALL PASS")
	os.Exit(0)
}
