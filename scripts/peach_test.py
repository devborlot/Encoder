#!/usr/bin/env python3
"""
Teste end-to-end do fluxo Peach (engenharia reversa).

Uso:
    python peach_test.py login                       # Só login + validação
    python peach_test.py lookup <termo>              # Busca anunciante
    python peach_test.py init <video.mxf> [opts]     # add_spot_upload_action (retorna creds STS)
    python peach_test.py upload <video.mxf> [opts]   # Fluxo completo: login -> init -> S3 multipart
    python peach_test.py session                     # Só validar session_data

Credenciais:
    Defina via env var:
        PEACH_EMAIL=...
        PEACH_PASSWORD=...
    OU passe via --email / --password
"""
import argparse
import json
import os
import re
import sys
from pathlib import Path

import requests

BASE = "https://latam.peachvideo.com"
USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:149.0) Gecko/20100101 Firefox/149.0"


def make_session():
    s = requests.Session()
    s.headers.update({
        "User-Agent": USER_AGENT,
        "Accept-Language": "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7",
    })
    return s


def login(session: requests.Session, email: str, password: str) -> bool:
    """Faz login e retorna True se a sessão estiver ativa."""
    print(f"[login] POST /login/login/login (email={email})")
    res = session.post(
        f"{BASE}/login/login/login",
        data={
            "user_email": email,
            "user_password": password,
            "country": "BR",
            "lang": "pt_BR",
        },
        headers={
            "Origin": BASE,
            "Referer": f"{BASE}/login/login/index?pais=BR",
        },
        allow_redirects=True,
    )
    print(f"[login] Status final: {res.status_code}")
    print(f"[login] URL final:    {res.url}")
    print(f"[login] Cookies setados:")
    for c in session.cookies:
        val = c.value if len(c.value) < 60 else c.value[:60] + "..."
        print(f"   {c.name}={val}")
    return validate_session(session)


def validate_session(session: requests.Session) -> bool:
    """Chama session_data.php e verifica se a sessão tá ativa."""
    print(f"\n[session] POST /app/comun/session_data.php")
    res = session.post(
        f"{BASE}/app/comun/session_data.php",
        headers={
            "X-Requested-With": "XMLHttpRequest",
            "Origin": BASE,
            "Referer": f"{BASE}/amasv/app/index_general.php",
        },
    )
    print(f"[session] Status: {res.status_code}")
    try:
        data = res.json()
    except json.JSONDecodeError:
        print(f"[session] ERRO: resposta não é JSON")
        print(res.text[:500])
        return False

    iniciada = data.get("iniciada")
    print(f"[session] iniciada: {iniciada}")
    if iniciada == 1:
        print(f"[session] usuário: {data.get('nombre_usuario_activo')} <{data.get('id_email')}>")
        print(f"[session] empresa: {data.get('empresa_nombre')} ({data.get('id_empresa')})")
        print(f"[session] privilégios: {data.get('privilegios')}")
        print(f"[session] extensões: {data.get('extension_permitida')}")
        return True
    else:
        print(f"[session] ❌ sessão NÃO está ativa")
        print(f"[session] resposta completa:\n{json.dumps(data, indent=2)[:1000]}")
        return False


def lookup_avisador(session: requests.Session, q: str):
    print(f"\n[lookup] Buscando anunciante: '{q}'")
    res = session.get(
        f"{BASE}/amasv/app/comun/AviMarPro2.php",
        params={"item": "avisador", "id_avisador": "", "id_marca": "", "elecciones": "0", "q": q, "page": "1"},
        headers={"X-Requested-With": "XMLHttpRequest"},
    )
    data = res.json()
    avisadores = data.get("data", {}).get("avisadores", [])
    print(f"[lookup] Encontrados: {len(avisadores)}")
    for a in avisadores[:5]:
        print(f"   {a['avisador_ID_EMPRESA']:12} {a['avisador_NOMBRE']}")
    return avisadores


def lookup_cnpj(session: requests.Session, id_empresa: str):
    print(f"\n[lookup] CNPJ de {id_empresa}")
    res = session.get(
        f"{BASE}/app/comun/busca_CNPJ.php",
        params={"accion": "buscar", "id_empresa": id_empresa},
        headers={"X-Requested-With": "XMLHttpRequest"},
    )
    print(f"[lookup] CNPJ: {res.text}")
    return res.json()


def lookup_marcas(session: requests.Session, id_avisador: str):
    print(f"\n[lookup] Marcas de {id_avisador}")
    res = session.get(
        f"{BASE}/amasv/app/comun/AviMarPro2.php",
        params={"item": "marca", "id_avisador": id_avisador, "id_marca": "", "elecciones": "0", "q": "", "page": "1"},
        headers={"X-Requested-With": "XMLHttpRequest"},
    )
    marcas = res.json().get("data", {}).get("marcas", [])
    for m in marcas:
        print(f"   {m['marca_ID_MARCA']:8} {m['marca_NOMBRE']}")
    return marcas


def lookup_produtos(session: requests.Session, id_avisador: str, id_marca):
    print(f"\n[lookup] Produtos de marca {id_marca}")
    res = session.get(
        f"{BASE}/amasv/app/comun/AviMarPro2.php",
        params={"item": "producto", "id_avisador": id_avisador, "id_marca": id_marca, "elecciones": "0", "q": "", "page": "1"},
        headers={"X-Requested-With": "XMLHttpRequest"},
    )
    produtos = res.json().get("data", {}).get("productos", [])
    for p in produtos:
        print(f"   {p['producto_ID_PRODUCTO']:8} {p['producto_NOMBRE']}")
    return produtos


def init_upload(session: requests.Session, params: dict) -> dict:
    """
    Chama add_spot_upload_action.php e parseia a resposta HTML/JS
    pra extrair credenciais STS e info do envio.
    """
    print(f"\n[init] GET /amasv/app/modulos/subir/add_spot_upload_action.php")
    print(f"[init] params:")
    for k, v in params.items():
        print(f"   {k}={v}")
    res = session.get(
        f"{BASE}/amasv/app/modulos/subir/add_spot_upload_action.php",
        params=params,
        headers={
            "X-Requested-With": "XMLHttpRequest",
            "Referer": f"{BASE}/amasv/app/index_general.php",
        },
    )
    print(f"[init] Status: {res.status_code}")
    html = res.text

    # Parsing: extrai os campos do bloco Filetransfer.upload({...})
    fields = {}
    patterns = {
        "id_envio": r"id_envio\s*:\s*['\"]([^'\"]+)['\"]",
        "destination": r"destination\s*:\s*['\"]([^'\"]+)['\"]",
        "region": r"region\s*:\s*['\"]([^'\"]+)['\"]",
        "Bucket": r"Bucket\s*:\s*['\"]([^'\"]+)['\"]",
        "AccessKeyId": r"AccessKeyId\s*:\s*['\"]([^'\"]+)['\"]",
        "SecretAccessKey": r"SecretAccessKey\s*:\s*['\"]([^'\"]+)['\"]",
        "SessionToken": r"SessionToken\s*:\s*['\"]([^'\"]+)['\"]",
        "upload_type": r"upload_type\s*:\s*['\"]([^'\"]+)['\"]",
    }
    for key, pat in patterns.items():
        m = re.search(pat, html)
        if m:
            v = m.group(1)
            fields[key] = v
            short = v if len(v) < 50 else v[:50] + "..."
            print(f"   {key}: {short}")
        else:
            print(f"   {key}: NÃO ENCONTRADO")

    if "AccessKeyId" not in fields:
        print(f"[init] ⚠️ resposta inesperada (primeiros 1000 chars):")
        print(html[:1000])

    return fields


def s3_multipart_upload(file_path: Path, sts: dict):
    """Upload S3 multipart usando credenciais STS retornadas."""
    try:
        import boto3
    except ImportError:
        print("[s3] ⚠️ boto3 não instalado. Instale com: pip install boto3")
        return False

    print(f"\n[s3] Iniciando upload de {file_path.name} ({file_path.stat().st_size:,} bytes)")
    print(f"[s3] Bucket: {sts['Bucket']}")
    print(f"[s3] Key: {sts['destination']}")

    s3 = boto3.client(
        "s3",
        region_name=sts["region"],
        aws_access_key_id=sts["AccessKeyId"],
        aws_secret_access_key=sts["SecretAccessKey"],
        aws_session_token=sts["SessionToken"],
    )

    from boto3.s3.transfer import TransferConfig
    cfg = TransferConfig(multipart_threshold=5 * 1024 * 1024, multipart_chunksize=5 * 1024 * 1024)

    def progress(bytes_sent):
        progress.total += bytes_sent
        pct = progress.total * 100 / file_path.stat().st_size
        print(f"\r[s3] {progress.total:,} / {file_path.stat().st_size:,} bytes ({pct:.1f}%)", end="", flush=True)
    progress.total = 0

    try:
        s3.upload_file(
            str(file_path),
            sts["Bucket"],
            sts["destination"],
            Config=cfg,
            Callback=progress,
        )
        print(f"\n[s3] ✅ upload concluído")
        return True
    except Exception as e:
        print(f"\n[s3] ❌ erro: {e}")
        return False


# ----------------- CLI -----------------

def cmd_login(args):
    s = make_session()
    if login(s, args.email, args.password):
        print("\n✅ LOGIN OK")
    else:
        print("\n❌ LOGIN FALHOU")
        sys.exit(1)


def cmd_session(args):
    s = make_session()
    if not login(s, args.email, args.password):
        sys.exit(1)
    if not validate_session(s):
        sys.exit(1)


def cmd_lookup(args):
    s = make_session()
    if not login(s, args.email, args.password):
        sys.exit(1)
    avs = lookup_avisador(s, args.q)
    if avs:
        first = avs[0]
        lookup_cnpj(s, first["avisador_ID_EMPRESA"])
        marcas = lookup_marcas(s, first["avisador_ID_EMPRESA"])
        if marcas:
            lookup_produtos(s, first["avisador_ID_EMPRESA"], marcas[0]["marca_ID_MARCA"])


def cmd_init(args):
    s = make_session()
    if not login(s, args.email, args.password):
        sys.exit(1)

    video = Path(args.video)
    if not video.exists():
        print(f"❌ Vídeo não existe: {video}")
        sys.exit(1)

    # Hardcoded como no HAR — depois vamos parametrizar
    params = {
        "v": "1",
        "pieza": video.stem,
        "AvisadorExtranjero": "0",
        "avisador": args.avisador or "BRA0743",
        "CNPJ_Avisador": args.cnpj_avisador or "30689848000130",
        "id_marca": args.id_marca or "9758",
        "id_producto": args.id_producto or "25322",
        "campana": "",
        "codigo": args.codigo or "20240174220243",
        "tipoCRT": "A",
        "AgenciaExtranjero": "0",
        "AgenciaCreativa": args.agencia or "BR0741",
        "CNPJ_Creativa": args.cnpj_agencia or "01936260000136",
        "productora": args.productora or "BRP190418",
        "archivo": video.name,
        "formato": args.formato or "2",
        "aspecto": "",
        "framerate": args.framerate or "29.97",
        "horas": "00",
        "minutos": "00",
        "segundos": str(args.duracao or 15).zfill(2),
        "frame": "00",
        "PosInicio": "07",
        "Vineta": "no",
        "ClosedCaption": "no",
        "TeclaSap": "no",
        "LenguajeSenas": "no",
        "AD": "0",
        "surround": "0",
        "audio": "stereo",
        "envio_exhibidor_bloqueado": "0",
        "elecciones": "0",
        "NotificarEmails": "",
    }
    sts = init_upload(s, params)
    if "AccessKeyId" in sts:
        print("\n✅ INIT OK — credenciais STS obtidas")
    else:
        print("\n❌ INIT FALHOU")
        sys.exit(1)


def cmd_upload(args):
    s = make_session()
    if not login(s, args.email, args.password):
        sys.exit(1)

    video = Path(args.video)
    if not video.exists():
        print(f"❌ Vídeo não existe: {video}")
        sys.exit(1)

    params = {
        "v": "1",
        "pieza": video.stem,
        "AvisadorExtranjero": "0",
        "avisador": args.avisador or "BRA0743",
        "CNPJ_Avisador": args.cnpj_avisador or "30689848000130",
        "id_marca": args.id_marca or "9758",
        "id_producto": args.id_producto or "25322",
        "campana": "",
        "codigo": args.codigo or "20240174220243",
        "tipoCRT": "A",
        "AgenciaExtranjero": "0",
        "AgenciaCreativa": args.agencia or "BR0741",
        "CNPJ_Creativa": args.cnpj_agencia or "01936260000136",
        "productora": args.productora or "BRP190418",
        "archivo": video.name,
        "formato": args.formato or "2",
        "aspecto": "",
        "framerate": args.framerate or "29.97",
        "horas": "00",
        "minutos": "00",
        "segundos": str(args.duracao or 15).zfill(2),
        "frame": "00",
        "PosInicio": "07",
        "Vineta": "no",
        "ClosedCaption": "no",
        "TeclaSap": "no",
        "LenguajeSenas": "no",
        "AD": "0",
        "surround": "0",
        "audio": "stereo",
        "envio_exhibidor_bloqueado": "0",
        "elecciones": "0",
        "NotificarEmails": "",
    }
    sts = init_upload(s, params)
    if "AccessKeyId" not in sts:
        print("\n❌ INIT falhou — abortando upload")
        sys.exit(1)

    print("\n⚠️ ATENÇÃO: o próximo passo vai fazer upload REAL do arquivo pra Peach.")
    if input("Continuar? [s/N]: ").strip().lower() != "s":
        print("Cancelado.")
        return

    s3_multipart_upload(video, sts)
    print("\n[done] Upload concluído. O Peach vai processar automaticamente.")
    print("[done] Verifique no portal latam.peachvideo.com se aparece em 'Subir'.")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--email", default=os.environ.get("PEACH_EMAIL"))
    parser.add_argument("--password", default=os.environ.get("PEACH_PASSWORD"))
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("login")
    sub.add_parser("session")

    p_lookup = sub.add_parser("lookup")
    p_lookup.add_argument("q")

    for name in ("init", "upload"):
        p = sub.add_parser(name)
        p.add_argument("video")
        p.add_argument("--avisador")
        p.add_argument("--cnpj-avisador")
        p.add_argument("--id-marca")
        p.add_argument("--id-producto")
        p.add_argument("--agencia")
        p.add_argument("--cnpj-agencia")
        p.add_argument("--productora")
        p.add_argument("--codigo")
        p.add_argument("--formato")
        p.add_argument("--framerate")
        p.add_argument("--duracao", type=int)

    args = parser.parse_args()

    if not args.email or not args.password:
        print("❌ Defina PEACH_EMAIL e PEACH_PASSWORD via env var ou --email/--password")
        sys.exit(2)

    fns = {
        "login": cmd_login,
        "session": cmd_session,
        "lookup": cmd_lookup,
        "init": cmd_init,
        "upload": cmd_upload,
    }
    fns[args.cmd](args)


if __name__ == "__main__":
    main()
