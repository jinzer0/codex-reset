# Codex Reset

[English](README.md) · [릴리스](https://github.com/jinzer0/codex-reset/releases)

지정한 시각에 **Codex 사용량 초기화**를 한 번 시도하도록 예약하는 명령줄 도구입니다.

- **지원 환경** — macOS (Apple Silicon·Intel), Ubuntu/Linux x86_64
- **Codex CLI** — `0.165.0` 또는 `0.166.0`
- **계정** — Codex CLI에 로그인한 ChatGPT 계정과 사용 가능한 초기화 크레딧

> 실험 단계의 비공식 프로젝트입니다. 실제 계정에서 크레딧을 사용한 초기화는 아직 테스트하지 않았습니다. [검증 내용 →](docs/RUST_VERIFICATION.md)

## 설치

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0-alpha.1/install.sh
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

# 내일 오전 4시 ('tomorrow 04:00'도 사용 가능)
codex-reset schedule --at '내일 04:00'

# 오늘 오전 4시 (이미 지난 시간이면 예약 불가)
codex-reset schedule --at 'today 04:00'

# 날짜와 시간 지정
codex-reset schedule --at '2026-01-01 04:00'
```

- **시간대** — `--timezone`을 생략하면 시스템에서 감지한 시간대를 사용하며, 감지하지 못하면 UTC를 사용합니다. 한국 시간은 `--timezone Asia/Seoul`로 지정하세요.
- **예약 시각** — 현재 시각으로부터 **최소 60초 이후**여야 합니다. 날짜 예시는 원하는 미래 날짜로 바꾸세요.

### 예약 승인

1. **내용 확인** — `schedule` 실행 직후 계정과 예약 시각을 표시합니다.
2. **실행 승인·예약 등록** — `Authorize one real reset at this time? [y/N]:`에 `y` 또는 `yes`로 답하면 **지정 시각의 초기화 실행을 승인하고 예약을 등록**합니다. 그 외 입력이나 Enter는 등록하지 않습니다.
3. **예약 시각에 실행** — 표시된 계정의 초기화를 **추가 확인 없이 한 번 시도**합니다.

| 실행 방식 | 예약 등록 | 초기화 시도 |
| --- | --- | --- |
| 기본 `schedule` | 위 확인 질문에 승인한 뒤 등록 | 지정 시각에 한 번 시도 |
| `--dry-run` 추가 | 계정·시각만 미리 확인, 등록하지 않음 | 실행하지 않음 |
| `--yes` 추가 | 확인 질문 없이 등록 — 스크립트용 | 지정 시각에 한 번 시도 |

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

> **`completed`만으로 초기화 성공을 뜻하지는 않습니다.**
> `completed`는 처리 결과가 기록되었다는 뜻이며, 초기화 요청 전에 확인한 `noCredit`도 포함합니다. `reset`은 서버가 초기화를 수행했다고 응답한 경우입니다.
> `status`의 서버 응답과 `Usage verification`(사용량 검증 결과)을 함께 확인하세요.

초기화 시도 전후의 사용량과 상세 오류는 `codex-reset status --json`으로 확인할 수 있습니다. [결과별 의미 →](docs/RESET_VERIFICATION.md)

## 예약 실행과 취소

- **전원·절전** — 예약 시각에 컴퓨터가 켜져 있고 절전 상태가 아니어야 합니다.
- **Ubuntu 로그아웃** — 로그아웃 후에도 실행하려면 `lingering`이 활성화되어 있어야 합니다. 설정을 확인할 수 없으면 예약 등록 결과에 경고가 표시됩니다.
- **지연·재시도** — 예약 시각보다 **5초 넘게 늦어지면 요청을 보내지 않습니다.** 초기화 요청은 예약당 최대 한 번이며, 오류나 응답 누락이 발생해도 다시 보내지 않습니다.
- **Codex 업그레이드** — 기존 예약이 실행되지 않을 수 있으므로 취소 후 다시 예약하세요.
- **취소 시점** — `pending`(대기 중) 상태에서 `cancel`을 실행하세요. 실행이 시작된 예약은 취소할 수 없습니다. 로그아웃하거나 계정을 전환해도 예약이 계속 실행될 수 있습니다.

## 라이선스

[MIT](LICENSE). 타사 구성 요소에는 각 라이선스가 적용됩니다. 관련 고지는 [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt)와 릴리스 압축 파일에 포함되어 있습니다.
