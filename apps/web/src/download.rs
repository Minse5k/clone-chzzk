//! 데스크톱 클라이언트 다운로드 페이지(`/download`).
//!
//! 파일은 GitHub Release에 있다. `releases/latest/download/<파일 이름>`은 가장 최근 릴리스의 같은 이름 파일로
//! 이어지므로, 새 버전을 올려도 이 페이지를 고칠 필요가 없다(파일 이름은 `.github/workflows/desktop.yml`이 고정한다).

use dioxus::prelude::*;

const RELEASES_URL: &str = "https://github.com/Minse5k/clone-chzzk/releases";
const LATEST_DOWNLOAD: &str = "https://github.com/Minse5k/clone-chzzk/releases/latest/download";

struct Package {
    label: &'static str,
    file: &'static str,
    note: &'static str,
}

struct Platform {
    os: &'static str,
    arch: &'static str,
    requirement: &'static str,
    packages: &'static [Package],
    install: &'static [&'static str],
}

const PLATFORMS: &[Platform] = &[
    Platform {
        os: "Windows",
        arch: "x64 (x86_64)",
        requirement: "Windows 10 이상. 화면은 WebView2 런타임으로 그린다(Windows 11과 최신 Windows 10에는 기본으로 있다).",
        packages: &[
            Package {
                label: "설치 프로그램 (.exe)",
                file: "chzzk-desktop-windows-x64-setup.exe",
                note: "권장. WebView2가 없으면 설치 중에 함께 설치한다.",
            },
            Package {
                label: "MSI 설치 파일 (.msi)",
                file: "chzzk-desktop-windows-x64.msi",
                note: "관리 도구로 배포할 때.",
            },
        ],
        install: &[
            "chzzk-desktop-windows-x64-setup.exe를 받아 실행한다.",
            "서명하지 않은 앱이라 \"Windows의 PC 보호\"(SmartScreen) 창이 뜬다. \"추가 정보\" → \"실행\"을 누른다.",
            "설치가 끝나면 시작 메뉴의 ChzzkDesktop을 연다.",
            "왼쪽에 방송 목록이 뜨면 정상이다. 방송을 누르면 오른쪽에서 재생된다.",
        ],
    },
    Platform {
        os: "macOS",
        arch: "Apple Silicon (arm64, M1 이후)",
        requirement: "macOS 11 Big Sur 이상. Intel Mac은 아직 패키지가 없다.",
        packages: &[
            Package {
                label: "디스크 이미지 (.dmg)",
                file: "chzzk-desktop-macos-arm64.dmg",
                note: "권장.",
            },
            Package {
                label: "앱 압축 파일 (.app.zip)",
                file: "chzzk-desktop-macos-arm64.app.zip",
                note: "dmg를 열 수 없을 때.",
            },
        ],
        install: &[
            "chzzk-desktop-macos-arm64.dmg를 열고 ChzzkDesktop을 응용 프로그램 폴더로 끌어 놓는다.",
            "서명·공증하지 않은 앱이라 처음 열면 \"확인되지 않은 개발자\" 경고가 뜬다. 터미널에서 xattr -dr com.apple.quarantine /Applications/ChzzkDesktop.app 을 실행하거나, 시스템 설정 → 개인정보 보호 및 보안에서 \"그래도 열기\"를 누른다.",
            "응용 프로그램 폴더에서 ChzzkDesktop을 연다.",
            "왼쪽에 방송 목록이 뜨면 정상이다. 방송을 누르면 오른쪽에서 재생된다.",
        ],
    },
];

#[component]
pub fn DownloadPage() -> Element {
    rsx! {
        h1 { "데스크톱 앱 다운로드" }
        p { class: "hint",
            "브라우저 없이 방송 목록을 보고 라이브·녹화 방송을 재생하는 앱이다. 기본으로 이 서버(https://mins.work)에 연결하고, 앱 위쪽의 서버 칸에서 주소를 바꿀 수 있다."
        }
        for platform in PLATFORMS {
            section { class: "download-platform",
                h2 { "{platform.os}" }
                p { class: "download-arch", "지원 아키텍처: {platform.arch}" }
                p { class: "hint", "{platform.requirement}" }
                ul { class: "download-files",
                    for package in platform.packages {
                        li {
                            a {
                                class: "button",
                                href: "{LATEST_DOWNLOAD}/{package.file}",
                                "{package.label}"
                            }
                            span { class: "hint", " {package.file} · {package.note}" }
                        }
                    }
                }
                h3 { "설치하고 실행하기" }
                ol { class: "download-steps",
                    for step in platform.install {
                        li { "{step}" }
                    }
                }
            }
        }
        section { class: "download-platform",
            h2 { "설치 없이 확인하기" }
            p {
                "설치한 앱이 서버에 닿는지는 창 없이도 확인할 수 있다. 방송 목록과 방송마다 재생할 주소를 출력하고 끝난다."
            }
            pre { class: "download-code",
                "Windows: \"%LOCALAPPDATA%\\ChzzkDesktop\\chzzk-desktop.exe\" --check\n"
                "macOS:   /Applications/ChzzkDesktop.app/Contents/MacOS/chzzk-desktop --check"
            }
        }
        p { class: "hint",
            "이전 버전과 변경 내역: "
            a { href: RELEASES_URL, "GitHub Releases" }
        }
    }
}
