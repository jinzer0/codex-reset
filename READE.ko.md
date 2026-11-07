# Codex Reset

[English](README.md) · [릴리스](https://github.com/jinzer0/codex-reset/releases)

지정한 시각에 **Codex 사용량 초기화**를 예약하는 명령줄 도구입니다.

- **지원 환경** — macOS (Apple Silicon·Intel), Ubuntu/Linux x86_64
- **Codex CLI** — `0.165.0`, `0.166.0` 또는 `0.167.0`
- **계정** — Codex CLI에 로그인한 ChatGPT 계정과 사용 가능한 초기화 크레딧

> [!NOTE]
> 비공식 프로젝트입니다. Codex CLI `0.167.0`으로 Ubuntu x86_64와 macOS Apple Silicon에서 실제 예약 실행, 사용량 초기화와 리셋권 차감을 확인했습니다. [검증 내용 →](docs/RUST_VERIFICATION.md)

## 설치

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0/install.sh
sh install.sh
```

## 사용법

```sh
codex-reset
```

### 직접 시간 지정

```sh
# 다음에 돌아오는 오전 4시 (오늘 또는 내일)
codex-reset schedule --at '04:00'

# 내일 오전 4시
codex-reset schedule --at '내일 04:00'
```

- **시간대** — 한국 시간은 `--timezone Asia/Seoul`로 지정하세요.
- **예약 시각** — 현재 시각으로부터 **최소 60초 이후**여야 합니다.

### 예약 승인

`schedule`이 표시하는 계정과 예약 시각을 확인하고 `y` 또는 `yes`로 승인하면 예약이 등록됩니다.

- `--dry-run`은 계정과 시각을 미리 확인합니다.
- `--yes`는 확인 질문 없이 예약을 등록합니다.

### 조회와 취소

```sh
# 로그인 계정과 남은 초기화 크레딧 조회
codex-reset check

# 예약 목록과 처리 결과 조회
codex-reset status

# 지정한 예약 취소
codex-reset cancel JOB_ID
```

`JOB_ID`에는 `status` 출력 첫 열에 표시된 예약 ID를 입력하세요.

초기화 시도 전후의 사용량과 상세 오류는 `codex-reset status --json`으로 확인할 수 있습니다. [결과별 의미 →](docs/RESET_VERIFICATION.md)

## 예약 실행과 취소

- **전원·절전** — 예약 시각에 컴퓨터가 켜져 있고 절전 상태가 아니어야 합니다.
- **Ubuntu 로그아웃** — 로그아웃 후에도 실행하려면 `lingering`이 활성화되어 있어야 합니다.
- **지연·재시도** — 예약 시각보다 **5초 넘게 늦어지면 요청을 보내지 않습니다.** 초기화 요청은 예약당 최대 한 번입니다.
- **Codex 업그레이드** — 업그레이드 후에는 기존 예약을 취소하고 다시 예약하세요.
- **취소 시점** — `pending`(대기 중) 상태에서 `cancel`을 실행하세요.

## 라이선스

[Unlicense](LICENSE) · [타사 라이선스 고지](THIRD_PARTY_NOTICES.txt)
