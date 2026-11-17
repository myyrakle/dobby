# dobby

로컬 개발 환경 청소부. macOS / Linux.

- **worktree**: 머지가 끝난 git worktree (Orca 관리 worktree 포함)
- **artifact**: 오래 안 건드린 프로젝트의 빌드 산출물 (`target/`, `node_modules/`, `.next/`, `__pycache__/`, `.venv/` 등)
- **cache**: 패키지 매니저 캐시 (cargo, go, npm, pnpm, yarn, bun, uv, pip, homebrew, gradle, maven, Xcode DerivedData)
- **docker**: 빌드 캐시, dangling/미사용 이미지, 정지된 컨테이너, 미사용 볼륨
- **memory**: 메모리 사용 현황, 파일 시스템 캐시 비우기

## 설치

```sh
cargo install --path .
```

## 사용법

```sh
dobby scan                       # 무엇을 지울 수 있는지 보기 (읽기 전용)
dobby scan -c worktree --json
dobby clean                      # dry run: safe 항목 중 지울 것 보기
dobby clean --yes                # safe 항목 삭제
dobby clean --yes --include-review
dobby clean --yes --id cache:go-mod --id docker:unused-images
dobby memory [--purge]
```

### 안전 등급

| 등급 | 의미 | 언제 지워지나 |
|------|------|---------------|
| `safe` | 재생성 가능, 잃을 것 없음 | `clean --yes`, 스케줄 실행 |
| `review` | 재다운로드 비용이 크거나 판단 필요 (캐시 전체 삭제, venv, docker 볼륨 등) | `--include-review` 또는 `--id` |
| `blocked` | 찾았지만 지우지 않음 (커밋 안 된 변경, locked) | 절대 안 지움 |

원칙: 쓰지 않는 것만 지우는 prune 계열(`pnpm store prune`, `uv cache prune`, `brew cleanup`)은 safe, 캐시를 통째로 지우는 것은 review.

### worktree 머지 판정

다음 중 하나가 있어야 "머지됨"으로 본다. 둘 다 없으면 진행 중인 작업으로 보고 건드리지 않는다.

1. **GitHub PR 이 merged** 이고 worktree 의 HEAD 가 그 PR 의 head 커밋과 같거나 그 조상 (squash/rebase 머지 대응, `gh` 필요)
2. 원격 브랜치가 삭제됐고(`[gone]`) HEAD 가 기본 브랜치의 조상

추가로 커밋 안 된 변경이 있으면 blocked, 마지막 활동이 `min_age_hours` 이내면 review 로 내린다.
삭제 직전에 dirty 여부를 다시 확인한다. Orca 가 관리하는 worktree 는 `orca worktree rm` 으로 지워서 Orca 상태와 어긋나지 않게 한다.

### 건너뛰기

프로젝트 디렉토리에 `.dobbyignore` 파일을 두면 그 아래는 스캔하지 않는다. 또는 설정의 `exclude` 에 경로를 넣는다.

## 설정

`~/.config/dobby/config.toml` (`dobby config init` 으로 생성, `dobby config show` 로 현재 값 확인)

```toml
roots = ["~/orca", "~/Codes"]   # 산출물/저장소를 찾을 곳
max_depth = 6
exclude = []

[artifacts]
min_age_days = 14               # 이만큼 안 건드린 프로젝트의 산출물만
min_size_mb = 1

[worktrees]
fetch = true                    # 판정 전 git fetch --prune
use_gh = true
delete_branch = true            # 머지 확인된 로컬 브랜치도 삭제
min_age_hours = 24

[caches]
disabled = []                   # 예: ["go-mod", "gradle"]

[docker]
min_age_hours = 168
```

## 자동 실행

```sh
dobby schedule install --at 12:30   # 매일 dobby clean --auto (safe 항목만)
dobby schedule status
dobby schedule uninstall
```

macOS 는 launchd agent(`~/Library/LaunchAgents/dev.dobby.gc.plist`, 로그 `~/Library/Logs/dobby.log`),
Linux 는 systemd user timer(로그 `~/.local/state/dobby/dobby.log`). 설치 시점의 `PATH` 를 실제 경로로 풀어서 저장하므로
fnm/nvm 등으로 깔린 도구도 찾는다. 도구를 새로 설치했으면 `install` 을 다시 실행한다.

## MCP

```sh
claude mcp add --scope user dobby -- dobby mcp
```

도구:

- `dobby_scan` — 읽기 전용 스캔
- `dobby_clean` — 기본 dry run. `dry_run: false` 로 실제 삭제. `ids` 로 개별 지정 가능 (review 포함)
- `dobby_memory` — 메모리 현황, `purge: true` 로 캐시 비우기

## 메모리

`dobby memory` 는 전체/가용 메모리와 상위 프로세스를 보여준다. `--purge` 는 파일 시스템 캐시를 비우는데
(macOS `purge`, Linux `drop_caches`) root 권한이 필요해서 `sudo -n` 으로만 시도한다. 비밀번호 없이 쓰려면 sudoers 에 해당 명령만 허용한다.

```
# macOS: /etc/sudoers.d/dobby
youruser ALL=(root) NOPASSWD: /usr/sbin/purge
```
