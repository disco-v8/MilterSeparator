<?php
$uuid = $_GET["uuid"] ?? "";
if ($uuid === "") exit;

$dbtype = "{{db_type}}";
if ($dbtype === "sqlite") {
    $pdo = new PDO("sqlite:{{db_path}}");
} elseif ($dbtype === "mysql") {
    $pdo = new PDO("mysql:host={{db_host}};dbname={{db_name}};charset=utf8mb4", "{{db_user}}", "{{db_password}}");
} elseif ($dbtype === "postgres") {
    $pdo = new PDO("pgsql:host={{db_host}};port={{db_port}};dbname={{db_name}}", "{{db_user}}", "{{db_password}}");
}

$stmt = $pdo->prepare("UPDATE download_tbl SET download_count = download_count + 1 WHERE uuid = ?");
$stmt->execute([$uuid]);
?>
