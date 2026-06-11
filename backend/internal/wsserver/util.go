package wsserver

import "time"

func timeNowMS() int64 {
	return time.Now().UnixMilli()
}
