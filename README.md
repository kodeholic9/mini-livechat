# Mini LiveChat (Project LiveCast)

초고성능 무전(PTT) 및 실시간 미디어 릴레이를 위한 경량 백엔드 서버 엔진입니다.
불필요한 오버헤드를 제거하고 Rust의 소유권(Ownership) 및 내부 가변성(Interior Mutability)을 극대화하여 엣지 디바이스 환경에서도 안정적으로 동작하도록 설계되었습니다.

## 아키텍처 핵심 포인트 (Design Philosophy)

- **Zero Memory Leak:** `Arc`와 `Weak` 포인터를 조합한 완벽한 순환 참조 차단.
- **Lock-Free 지향:** `RwLock` 승급 패턴과 패킷 단위 분할 `Mutex`를 사용하여 데이터 파이프라인 병목(Contention) 제거.
- **Symmetric RTP Latching:** STUN 서버 없이 UDP 패킷 출발지 주소를 실시간으로 갱신하여 0.1초 이내의 망 변경(Roaming) 지원.
- **Separation of Concerns:** 제어 평면(WebSocket)과 데이터 평면(UDP)의 철저한 분리. Member ID 기반의 식별과 SSRC 기반의 초고속 라우팅 매핑.

## 빌드 및 테스트 (Build & Test)

```bash
# 통합 테스트 실행 (메모리 누수 및 조립 검증)
cargo test

# 로깅과 함께 서버 실행 (Trace 레벨)
RUST_LOG=trace cargo run
```
