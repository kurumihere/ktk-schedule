package logger

import (
	"fmt"
	"log"
	"os"
	"strings"
	"sync/atomic"
)

type Level int32

const (
	LevelDebug Level = iota
	LevelInfo
	LevelWarn
	LevelError
)

var (
	currentLevel atomic.Int32
	output       = log.New(os.Stderr, "", 0)
)

func init() {
	SetLevel(LevelInfo)
}

func ParseLevel(value string) Level {
	switch strings.ToLower(strings.TrimSpace(value)) {
	case "debug":
		return LevelDebug
	case "warn", "warning":
		return LevelWarn
	case "error":
		return LevelError
	default:
		return LevelInfo
	}
}

func SetLevel(level Level) {
	currentLevel.Store(int32(level))
}

func Debug(format string, args ...any) { write(LevelDebug, "DEBUG", format, args...) }
func Info(format string, args ...any)  { write(LevelInfo, "INFO", format, args...) }
func Warn(format string, args ...any)  { write(LevelWarn, "WARN", format, args...) }
func Error(format string, args ...any) { write(LevelError, "ERROR", format, args...) }

func write(level Level, label, format string, args ...any) {
	if level < Level(currentLevel.Load()) {
		return
	}

	var line strings.Builder
	line.WriteString(label)
	line.WriteString(": ")
	fmt.Fprintf(&line, format, args...)
	output.Print(line.String())
}
