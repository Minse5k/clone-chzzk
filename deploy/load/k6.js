// clone-chzzk 부하 스크립트(과제 5). Caddy(리버스 프록시)를 거쳐 여러 app 인스턴스에 요청을 보낸다.
//
//   docker compose --profile load run --rm k6
//
// 환경변수: BASE_URL(기본 http://caddy), VUS(가상 사용자 수, 기본 40), DURATION(기본 3m).
// 방송 목록·상세·댓글 조회와 로그인을 섞는다. 로그인은 비밀번호 해시(argon2id)를 확인하느라 CPU를 많이 써서
// 인스턴스의 CPU 사용률을 올린다. 로그인 후 내 정보 조회로 토큰이 어느 인스턴스에서든 통하는지도 본다.
import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://caddy';
const PASSWORD = 'load-test-password';

export const options = {
  scenarios: {
    browse_and_login: {
      executor: 'constant-vus',
      vus: Number(__ENV.VUS || 40),
      duration: __ENV.DURATION || '3m',
    },
  },
  thresholds: {
    // 스케일 아웃·인 중에도 요청이 거의 실패하지 않아야 한다.
    http_req_failed: ['rate<0.01'],
  },
};

const json = { headers: { 'Content-Type': 'application/json' } };

// 부하용 사용자 하나를 가입시키고, 조회할 방송이 있으면 그 ID를 넘긴다.
export function setup() {
  const username = `load${Date.now() % 1000000000}`;
  const signup = http.post(
    `${BASE_URL}/api/auth/signup`,
    JSON.stringify({ username, password: PASSWORD }),
    json,
  );
  check(signup, { '부하용 사용자 가입 201': (r) => r.status === 201 });
  const broadcasts = http.get(`${BASE_URL}/api/broadcasts`).json();
  const broadcastId = broadcasts.length > 0 ? broadcasts[0].id : null;
  return { username, broadcastId };
}

export default function (data) {
  const list = http.get(`${BASE_URL}/api/broadcasts`);
  check(list, { '방송 목록 200': (r) => r.status === 200 });

  if (data.broadcastId) {
    const detail = http.get(`${BASE_URL}/api/broadcasts/${data.broadcastId}`);
    check(detail, { '방송 상세 200': (r) => r.status === 200 });
    const comments = http.get(`${BASE_URL}/api/broadcasts/${data.broadcastId}/comments`);
    check(comments, { '댓글 목록 200': (r) => r.status === 200 });
  }

  const login = http.post(
    `${BASE_URL}/api/auth/login`,
    JSON.stringify({ username: data.username, password: PASSWORD }),
    json,
  );
  if (check(login, { '로그인 200': (r) => r.status === 200 })) {
    const me = http.get(`${BASE_URL}/api/auth/me`, {
      headers: { Authorization: `Bearer ${login.json().token}` },
    });
    check(me, { '내 정보 200': (r) => r.status === 200 });
  }

  sleep(0.1);
}
